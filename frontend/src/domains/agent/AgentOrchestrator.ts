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
}

export interface BrowserInterface {
  getUrl: () => Promise<string>;
  getTitle: () => Promise<string>;
  getElements: () => Promise<InteractiveElement[]>;
  navigate: (url: string) => Promise<void>;
  click: (x: number, y: number) => Promise<void>;
  typeText: (x: number, y: number, text: string) => Promise<void>;
  pressKey: (key: string) => Promise<void>;
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
  private history: Array<{ thought: string; action: string; url: string; title: string; finished: boolean; answer: string }> = [];

  constructor(browser: BrowserInterface) {
    this.browser = browser;
  }

  public reset(): void {
    this.history = [];
  }

  public async runStep(goal: string, mode: 'simulation' | 'gemini' = 'simulation', apiKey?: string): Promise<AgentStepResult> {
    const url = await this.browser.getUrl();
    const title = await this.browser.getTitle();
    const elements = await this.browser.getElements();

    const domText = getSimplifiedDomText(elements);

    let stepResult: AgentStepResult;
    if (mode === 'simulation') {
      stepResult = await this.runSimulatedStep(goal, url, title, elements);
    } else {
      stepResult = await this.runGeminiStep(goal, url, title, domText, apiKey);
    }

    let executionMsg = '';
    let finished = false;
    let answer = '';

    try {
      if (stepResult.action === 'navigate' && stepResult.url) {
        executionMsg = `Navegando a ${stepResult.url}`;
        await this.browser.navigate(stepResult.url);
      } else if (stepResult.action === 'click' && typeof stepResult.target_id === 'number') {
        const el = elements.find((e) => e.id === stepResult.target_id);
        if (el) {
          const cx = Math.round(el.rect.x + el.rect.width / 2);
          const cy = Math.round(el.rect.y + el.rect.height / 2);
          executionMsg = `Haciendo clic en [${el.id}] ${el.tag_name} '${el.text}'`;
          await this.browser.click(cx, cy);
        } else {
          executionMsg = `Error: Elemento [${stepResult.target_id}] no encontrado`;
        }
      } else if (stepResult.action === 'type' && typeof stepResult.target_id === 'number') {
        const el = elements.find((e) => e.id === stepResult.target_id);
        if (el) {
          const cx = Math.round(el.rect.x + el.rect.width / 2);
          const cy = Math.round(el.rect.y + el.rect.height / 2);
          const text = stepResult.text || '';
          executionMsg = `Escribiendo '${text}' en [${el.id}] ${el.tag_name}`;
          await this.browser.typeText(cx, cy, text);
        } else {
          executionMsg = `Error: Campo de entrada [${stepResult.target_id}] no encontrado`;
        }
      } else if (stepResult.action === 'press' && stepResult.key) {
        executionMsg = `Presionando tecla '${stepResult.key}'`;
        await this.browser.pressKey(stepResult.key);
      } else if (stepResult.action === 'finish') {
        executionMsg = 'Objetivo completado';
        finished = true;
        answer = stepResult.answer || 'He terminado la tarea.';
      } else {
        executionMsg = 'Acción completada';
        finished = true;
      }
    } catch (err: any) {
      executionMsg = `Error al ejecutar acción: ${err.message || err}`;
      finished = true;
    }

    const stepData = {
      thought: stepResult.thought || '',
      action: stepResult.action,
      execution_msg: executionMsg,
      url,
      title,
      finished,
      answer
    };

    this.history.push({
      thought: stepData.thought,
      action: executionMsg,
      url,
      title,
      finished,
      answer
    });

    return stepData;
  }

  private async runSimulatedStep(goal: string, url: string, title: string, elements: InteractiveElement[]): Promise<AgentStepResult> {
    await new Promise((resolve) => setTimeout(resolve, 800));

    const goalLower = goal.toLowerCase();
    const stepCount = this.history.length;

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

  private async runGeminiStep(goal: string, url: string, title: string, domText: string, apiKey?: string): Promise<AgentStepResult> {
    if (!apiKey) {
      return {
        thought: 'Error: Se requiere una Gemini API Key.',
        action: 'finish',
        answer: 'Introduce tu Gemini API Key en Ajustes.'
      };
    }

    const systemPrompt = `Eres un agente autónomo de navegación web para un navegador nativo ultrarrápido.
Tu objetivo es interactuar con páginas web para cumplir la meta del usuario.

REGLAS DE ACCIÓN:
1. "navigate": {"action": "navigate", "url": "https://..."}
2. "click": {"action": "click", "target_id": <ID_NUMÉRICO>}
3. "type": {"action": "type", "target_id": <ID_NUMÉRICO>, "text": "texto"}
4. "press": {"action": "press", "key": "Enter"}
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
      const rawText = data.candidates?.[0]?.content?.parts?.[0]?.text || '{}';
      return JSON.parse(rawText) as AgentStepResult;
    } catch (err: any) {
      return {
        thought: `Error al consultar Gemini: ${err.message}`,
        action: 'finish',
        answer: `Error de conexión con Gemini: ${err.message}`
      };
    }
  }
}
