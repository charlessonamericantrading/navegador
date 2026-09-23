// -*- coding: utf-8 -*-
export interface ElementRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface InteractiveElement {
  id: number;
  tag_name: string;
  text: string;
  rect: ElementRect;
  selector: string;
  attributes: {
    id?: string;
    name?: string;
    placeholder?: string;
    element_type?: string;
    role?: string;
    href?: string;
    value?: string;
    checked?: boolean;
  };
}

export interface AgentStepResult {
  thought: string;
  action: 'navigate' | 'click' | 'type' | 'press' | 'finish' | string;
  target_id?: number;
  text?: string;
  key?: string;
  url?: string;
  answer?: string;
  execution_msg?: string;
  finished?: boolean;
  /** La acción no se ejecutó o el motor la rechazó. Nunca cuenta como éxito. */
  failed?: boolean;
}

interface ActionOutcome {
  message: string;
  finished: boolean;
  failed: boolean;
  answer: string;
}

/**
 * El motor rechazó un comando o no lo confirmó. Lo lanzan las
 * implementaciones de `BrowserInterface`; el orquestador lo convierte en un
 * paso `failed`.
 */
export class BrowserActionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BrowserActionError';
  }
}

export interface BrowserInterface {
  getUrl: () => Promise<string>;
  getTitle: () => Promise<string>;
  getElements: () => Promise<InteractiveElement[]>;
  getAccessibilityPrompt?: () => Promise<string>;
  navigate: (url: string) => Promise<void>;
  click: (x: number, y: number) => Promise<void>;
  typeText: (x: number, y: number, text: string) => Promise<void>;
  pressKey: (key: string) => Promise<void>;
}

/**
 * El usuario detuvo la ejecución. Se lanza en vez de devolver un paso para
 * que nadie lo confunda con un resultado: un paso cancelado no actuó.
 */
export class AgentCancelledError extends Error {
  constructor() {
    super('Ejecución del agente detenida');
    this.name = 'AgentCancelledError';
  }
}

/** Punto de control: tras cada espera, antes de seguir o de actuar. */
function throwIfCancelled(signal?: AbortSignal): void {
  if (signal?.aborted) throw new AgentCancelledError();
}

/** Espera que termina antes si se cancela, en vez de agotar el plazo. */
export function cancellableDelay(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new AgentCancelledError());
      return;
    }
    const onAbort = () => {
      clearTimeout(timer);
      reject(new AgentCancelledError());
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/**
 * Pseudo-acción para cuando el modelo no dio una decisión utilizable (sin
 * clave, error del proveedor, JSON roto o sin `action`). No es una acción
 * que el modelo pueda pedir con sentido: si la devolviera, también falla.
 */
const MODEL_ERROR_ACTION = 'model_error';

function modelError(message: string): AgentStepResult {
  return { thought: message, action: MODEL_ERROR_ACTION };
}

/**
 * Política ante fallos (decidida con el usuario, Fase 52): el fallo vuelve
 * al modelo en el historial para que corrija, pero dos pasos fallidos
 * seguidos detienen la ejecución. Tolera una página que cambia entre
 * observar y actuar sin dejar al agente repitiendo el mismo error.
 */
export const MAX_CONSECUTIVE_FAILURES = 2;

export function shouldStopAfterFailures(consecutiveFailures: number): boolean {
  return consecutiveFailures >= MAX_CONSECUTIVE_FAILURES;
}

export function getSimplifiedDomText(elements: InteractiveElement[]): string {
  const textLines: string[] = [];
  for (const el of elements) {
    const tag = el.tag_name.toLowerCase();
    const desc = el.text || '';
    const attrs = el.attributes || {};

    const details: string[] = [];
    if (attrs.placeholder) details.push(`placeholder="${attrs.placeholder}"`);
    if (attrs.element_type) details.push(`type="${attrs.element_type}"`);
    if (attrs.href) details.push(`href="${attrs.href}"`);
    if (attrs.name) details.push(`name="${attrs.name}"`);

    const detailsStr = details.length > 0 ? ` (${details.join(', ')})` : '';
    textLines.push(`[${el.id}] ${tag} "${desc}"${detailsStr}`);
  }
  return textLines.join('\n');
}

export class AgentOrchestrator {
  private browser: BrowserInterface;
  private history: Array<{ thought: string; kind: string; action: string; url: string; title: string; finished: boolean; answer: string }> = [];

  constructor(browser: BrowserInterface) {
    this.browser = browser;
  }

  public reset(): void {
    this.history = [];
  }

  /**
   * Observa la página, decide una acción y la ejecuta.
   *
   * `signal` cancela el paso: se comprueba tras cada espera y justo antes de
   * actuar, y llega a la petición al modelo. Una vez enviada una acción al
   * motor ya no se puede deshacer; lo que garantiza la cancelación es que no
   * se envía ninguna acción nueva después de detener.
   */
  public async runStep(goal: string, mode: 'simulation' | 'gemini' = 'simulation', apiKey?: string, signal?: AbortSignal): Promise<AgentStepResult> {
    throwIfCancelled(signal);
    const url = await this.browser.getUrl();
    const title = await this.browser.getTitle();
    const elements = await this.browser.getElements();
    throwIfCancelled(signal);

    let domText = '';
    if (this.browser.getAccessibilityPrompt) {
      try {
        domText = await this.browser.getAccessibilityPrompt();
      } catch {
        domText = getSimplifiedDomText(elements);
      }
    }
    if (!domText) {
      domText = getSimplifiedDomText(elements);
    }

    throwIfCancelled(signal);

    let stepResult: AgentStepResult;
    if (mode === 'simulation') {
      stepResult = await this.runSimulatedStep(goal, url, title, elements, signal);
    } else {
      stepResult = await this.runGeminiStep(goal, url, title, domText, apiKey, signal);
    }
    // La respuesta del modelo puede llegar después de pulsar «Detener»: no
    // se ejecuta.
    throwIfCancelled(signal);

    const outcome = await this.executeAction(stepResult, elements);

    const stepData = {
      thought: stepResult.thought || '',
      action: stepResult.action,
      execution_msg: outcome.message,
      url,
      title,
      finished: outcome.finished,
      failed: outcome.failed,
      answer: outcome.answer
    };

    this.history.push({
      thought: stepData.thought,
      kind: stepResult.action,
      // El modelo ve en el historial si el paso anterior fallo y por que:
      // es lo que le permite corregir en vez de repetir.
      action: outcome.failed ? `FALLÓ: ${outcome.message}` : outcome.message,
      url,
      title,
      finished: outcome.finished,
      answer: outcome.answer
    });

    return stepData;
  }

  /**
   * Ejecuta la acción decidida. Nunca da por buena una acción que no llegó a
   * ejecutarse (plan H15): elemento inexistente, acción desconocida o
   * malformada y error del motor son `failed`, no «completada».
   */
  private async executeAction(step: AgentStepResult, elements: InteractiveElement[]): Promise<ActionOutcome> {
    const fallo = (message: string): ActionOutcome => ({ message, finished: false, failed: true, answer: '' });
    const centro = (el: InteractiveElement) => [Math.round(el.rect.x + el.rect.width / 2), Math.round(el.rect.y + el.rect.height / 2)] as const;

    try {
      switch (step.action) {
        case 'navigate': {
          if (!step.url) return fallo('Acción navigate sin URL');
          await this.browser.navigate(step.url);
          return { message: `Navegando a ${step.url}`, finished: false, failed: false, answer: '' };
        }
        case 'click': {
          const el = elements.find((e) => e.id === step.target_id);
          if (!el) return fallo(`Elemento [${step.target_id}] no encontrado`);
          const [x, y] = centro(el);
          await this.browser.click(x, y);
          return { message: `Haciendo clic en [${el.id}] ${el.tag_name} '${el.text}'`, finished: false, failed: false, answer: '' };
        }
        case 'type': {
          const el = elements.find((e) => e.id === step.target_id);
          if (!el) return fallo(`Campo de entrada [${step.target_id}] no encontrado`);
          const [x, y] = centro(el);
          const text = step.text || '';
          await this.browser.typeText(x, y, text);
          return { message: `Escribiendo '${text}' en [${el.id}] ${el.tag_name}`, finished: false, failed: false, answer: '' };
        }
        case 'press': {
          if (!step.key) return fallo('Acción press sin tecla');
          await this.browser.pressKey(step.key);
          return { message: `Presionando tecla '${step.key}'`, finished: false, failed: false, answer: '' };
        }
        case 'finish':
          return { message: 'Objetivo completado', finished: true, failed: false, answer: step.answer || 'He terminado la tarea.' };
        case MODEL_ERROR_ACTION:
          return fallo(step.thought || 'El modelo no devolvió una acción válida');
        default:
          // Antes: «Acción completada» y fin de la tarea. Una respuesta que no
          // es ninguna acción conocida no completa nada.
          return fallo(`Acción desconocida: ${JSON.stringify(step.action)}`);
      }
    } catch (err) {
      return fallo(`Error al ejecutar acción: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  private async runSimulatedStep(goal: string, url: string, title: string, elements: InteractiveElement[], signal?: AbortSignal): Promise<AgentStepResult> {
    await cancellableDelay(800, signal);

    const goalLower = goal.toLowerCase();
    const stepCount = this.history.length;

    // Escribir ya no envía el formulario: enviarlo es una acción aparte.
    if (this.history[stepCount - 1]?.kind === 'type') {
      return {
        thought: 'He escrito la consulta; la envío pulsando Enter.',
        action: 'press',
        key: 'Enter'
      };
    }

    if (stepCount === 0) {
      if (goalLower.includes('wikipedia')) {
        return {
          thought: 'Para buscar información en Wikipedia, primero debo navegar a wikipedia.org.',
          action: 'navigate',
          url: 'https://wikipedia.org'
        };
      } else {
        return {
          thought: 'Para cumplir con la tarea, primero navegaré a Google para buscar información.',
          action: 'navigate',
          url: 'https://google.com'
        };
      }
    }

    if (url.includes('google.com')) {
      const searchInput = elements.find(
        (e) => e.tag_name === 'input' && (e.attributes.name === 'q' || e.text.toLowerCase().includes('search') || e.text.toLowerCase().includes('buscar'))
      );
      if (searchInput) {
        let query = goal;
        if (goalLower.includes('busca')) query = goal.split(/busca/i)[1].trim();
        else if (goalLower.includes('search')) query = goal.split(/search/i)[1].trim();
        return {
          thought: `Localizo el cuadro de búsqueda (ID ${searchInput.id}). Escribo: '${query}'.`,
          action: 'type',
          target_id: searchInput.id,
          text: query
        };
      }

      const firstResult = elements.find(
        (e) => e.tag_name === 'a' && e.attributes.href && !e.attributes.href.includes('google.com')
      );
      if (firstResult) {
        return {
          thought: `Selecciono el enlace relevante '${firstResult.text}' (ID ${firstResult.id}).`,
          action: 'click',
          target_id: firstResult.id
        };
      }
    }

    if (url.includes('wikipedia.org')) {
      if (url.includes('/wiki/')) {
        const paragraphs = elements.filter((e) => e.tag_name === 'a' && e.text.length > 25);
        const summary = `He llegado a ${title}. Contenido relevante: '${paragraphs[0]?.text || 'Artículo encontrado'}'.`;
        return {
          thought: `En el artículo de Wikipedia '${title}', extraigo la información solicitada.`,
          action: 'finish',
          answer: summary
        };
      }
    }

    if (stepCount >= 3) {
      return {
        thought: `He analizado la página '${title}' y recopilé la información necesaria.`,
        action: 'finish',
        answer: `Tarea completada en '${title}' (${url}) para el objetivo: '${goal}'.`
      };
    }

    const firstLink = elements.find((e) => e.tag_name === 'a' && e.text.length > 8);
    if (firstLink) {
      return {
        thought: `Explorando enlace '${firstLink.text}' (ID ${firstLink.id}).`,
        action: 'click',
        target_id: firstLink.id
      };
    }

    return {
      thought: 'No hay más elementos con los que interactuar.',
      action: 'finish',
      answer: 'Finalizado.'
    };
  }

  private async runGeminiStep(goal: string, url: string, title: string, domText: string, apiKey?: string, signal?: AbortSignal): Promise<AgentStepResult> {
    if (!apiKey) {
      return modelError('Se requiere una Gemini API Key: introdúcela en Ajustes.');
    }

    const systemPrompt = `Eres un agente autónomo de navegación web para un navegador nativo ultrarrápido.
Tu objetivo es interactuar con páginas web para cumplir la meta del usuario.

REGLAS DE ACCIÓN:
1. "navigate": {"action": "navigate", "url": "https://..."}
2. "click": {"action": "click", "target_id": <ID_NUMÉRICO>}
3. "type": {"action": "type", "target_id": <ID_NUMÉRICO>, "text": "texto"} (solo escribe; NO envía el formulario)
4. "press": {"action": "press", "key": "Enter"} (para enviar lo escrito, como paso aparte)
5. "finish": {"action": "finish", "answer": "respuesta final"}

RESPONDE EXCLUSIVAMENTE CON UN OBJETO JSON con las claves: thought, action, target_id, text, key, url, answer.`;

    const userPrompt = `OBJETIVO: ${goal}
ESTADO ACTUAL:
- URL: ${url}
- Título: ${title}
- Pasos previos: ${JSON.stringify(this.history.map((h) => ({ thought: h.thought, action: h.action })))}

ELEMENTOS INTERACTIVOS DISPONIBLES:
${domText}

Decide el siguiente paso y responde en JSON.`;

    try {
      const response = await fetch(
        `https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key=${apiKey}`,
        {
          method: 'POST',
          signal,
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            contents: [
              {
                role: 'user',
                parts: [{ text: systemPrompt + '\n\n' + userPrompt }]
              }
            ],
            generationConfig: {
              responseMimeType: 'application/json',
              temperature: 0.2
            }
          })
        }
      );

      if (!response.ok) {
        const errText = await response.text();
        throw new Error(`API error (${response.status}): ${errText}`);
      }

      const data = await response.json();
      let rawText = data.candidates?.[0]?.content?.parts?.[0]?.text || '{}';
      rawText = rawText.replace(/^```(?:json)?\s*/i, '').replace(/\s*```$/, '').trim();
      const parsed: unknown = JSON.parse(rawText);
      if (typeof parsed !== 'object' || parsed === null || typeof (parsed as AgentStepResult).action !== 'string') {
        throw new Error(`respuesta sin acción: ${rawText.slice(0, 200)}`);
      }
      return parsed as AgentStepResult;
    } catch (err) {
      // Una cancelación no es un error de conexión que deba acabar la tarea
      // con una «respuesta»: se propaga tal cual.
      throwIfCancelled(signal);
      // Antes esto era `finish` con el error como «respuesta»: un fallo del
      // proveedor o un JSON roto terminaban la tarea como si se hubiera
      // cumplido. Ahora es un paso fallido y cuenta para el tope de fallos.
      return modelError(`Error al consultar Gemini: ${err instanceof Error ? err.message : String(err)}`);
    }
  }
}
