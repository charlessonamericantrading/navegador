// Pruebas del orquestador del agente (plan F05, H16/H17). Corren con el
// test runner de Node sobre el `.ts` directamente: el proyecto ya exige
// `erasableSyntaxOnly`, así que Node 24 puede quitar los tipos sin compilar.
//
//   npm test
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { AgentCancelledError, AgentOrchestrator, type BrowserInterface, type InteractiveElement } from '../src/domains/agent/AgentOrchestrator.ts';

const buscador: InteractiveElement = {
  id: 1,
  tag_name: 'input',
  text: 'Buscar',
  rect: { x: 10, y: 10, width: 100, height: 20 },
  selector: 'input[name=q]',
  attributes: { name: 'q' },
};

/** Navegador falso que apunta cada acción que recibe. */
function navegadorFalso(url = 'https://google.com/', elementos: InteractiveElement[] = [buscador]) {
  const acciones: string[] = [];
  const browser: BrowserInterface = {
    getUrl: async () => url,
    getTitle: async () => 'Google',
    getElements: async () => elementos,
    navigate: async (u) => { acciones.push(`navigate ${u}`); },
    click: async (x, y) => { acciones.push(`click ${x},${y}`); },
    typeText: async (_x, _y, text) => { acciones.push(`type ${text}`); },
    pressKey: async (key) => { acciones.push(`press ${key}`); },
  };
  return { browser, acciones };
}

test('detener mientras el modelo decide no ejecuta ninguna acción', async () => {
  const { browser, acciones } = navegadorFalso();
  const agente = new AgentOrchestrator(browser);
  const controller = new AbortController();

  const paso = agente.runStep('busca gatos', 'simulation', undefined, controller.signal);
  setTimeout(() => controller.abort(), 50);

  await assert.rejects(paso, AgentCancelledError);
  assert.deepEqual(acciones, []);
});

test('una respuesta del modelo que llega tras detener no se ejecuta', async (t) => {
  const { browser, acciones } = navegadorFalso();
  const agente = new AgentOrchestrator(browser);
  const controller = new AbortController();

  // Un `fetch` que ignora la señal y responde tarde: el peor caso. La
  // respuesta pide navegar; si llegara a ejecutarse, quedaría apuntada.
  const respuesta = { candidates: [{ content: { parts: [{ text: JSON.stringify({ thought: 't', action: 'navigate', url: 'https://ejemplo.test/' }) }] } }] };
  t.mock.method(globalThis, 'fetch', async () => {
    controller.abort();
    return new Response(JSON.stringify(respuesta), { status: 200 });
  });

  await assert.rejects(agente.runStep('objetivo', 'gemini', 'clave', controller.signal), AgentCancelledError);
  assert.deepEqual(acciones, []);
});

test('la cancelación llega a la petición al modelo y no se disfraza de error de conexión', async (t) => {
  const { browser, acciones } = navegadorFalso();
  const agente = new AgentOrchestrator(browser);
  const controller = new AbortController();

  let senalRecibida: AbortSignal | undefined;
  t.mock.method(globalThis, 'fetch', (_url: string, init: RequestInit) => {
    senalRecibida = init.signal ?? undefined;
    return new Promise((_resolve, reject) => {
      init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
    });
  });

  const paso = agente.runStep('objetivo', 'gemini', 'clave', controller.signal);
  setTimeout(() => controller.abort(), 10);

  await assert.rejects(paso, AgentCancelledError);
  assert.equal(senalRecibida, controller.signal);
  assert.deepEqual(acciones, []);
});

test('con la señal ya abortada ni siquiera observa la página', async () => {
  let observada = false;
  const { browser } = navegadorFalso();
  browser.getUrl = async () => { observada = true; return ''; };
  const controller = new AbortController();
  controller.abort();

  await assert.rejects(new AgentOrchestrator(browser).runStep('x', 'simulation', undefined, controller.signal), AgentCancelledError);
  assert.equal(observada, false);
});

test('escribir y enviar son pasos distintos', async () => {
  const { browser, acciones } = navegadorFalso();
  const agente = new AgentOrchestrator(browser);

  await agente.runStep('busca gatos', 'simulation'); // navegar a Google
  await agente.runStep('busca gatos', 'simulation'); // escribir la consulta
  assert.deepEqual(acciones.slice(1), ['type gatos'], 'escribir no debe pulsar Enter por su cuenta');

  const envio = await agente.runStep('busca gatos', 'simulation');
  assert.equal(envio.action, 'press');
  assert.deepEqual(acciones.slice(1), ['type gatos', 'press Enter']);
});
