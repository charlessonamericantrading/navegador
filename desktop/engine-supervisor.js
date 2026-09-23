// Supervisor de renderers: un proceso `engine_server` por renderer (pestaña),
// etapa 1 de docs/adr/0001-broker-y-aislamiento-de-renderers.md.
//
// Lo que aporta: CONTENCIÓN. Si un renderer se cuelga, entra en pánico o lo
// mata el sistema, sus peticiones pendientes se rechazan con `EngineCrashedError`
// (no con un timeout de 30 s), se emite `crash` y los demás siguen igual.
// Lo que NO aporta todavía: confidencialidad. Cada renderer sigue haciendo su
// propia red y su propio disco (etapa 2).
//
// Interfaz deliberadamente pequeña —open, request, close, closeAll, on— para
// poder cambiar la implementación por un broker Rust sin tocar a quien la usa.
// Sin Electron: el arranque del proceso se inyecta (`spawnEngine`).

const { EventEmitter } = require('events');
const { createLineSplitter } = require('./engine-protocol');

class EngineCrashedError extends Error {
  constructor(rendererId, detail) {
    super(`El renderer ${rendererId} se cayó (${detail})`);
    this.name = 'EngineCrashedError';
    this.rendererId = rendererId;
  }
}

function createEngineSupervisor({ spawnEngine, maxLineChars = 128 * 1024 * 1024, maxPending = 64, requestTimeoutMs = 30_000, readyTimeoutMs = 20_000 }) {
  const events = new EventEmitter();
  const renderers = new Map();

  function fail(renderer, error) {
    for (const { reject, timer } of renderer.pending.values()) {
      clearTimeout(timer);
      reject(error);
    }
    renderer.pending.clear();
  }

  function handleLine(renderer, line) {
    if (!line.trim()) return;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      events.emit('protocol-error', { rendererId: renderer.id, line: line.slice(0, 100) });
      return;
    }
    if (msg.id && renderer.pending.has(msg.id)) {
      const { resolve, timer } = renderer.pending.get(msg.id);
      clearTimeout(timer);
      renderer.pending.delete(msg.id);
      resolve(msg);
      return;
    }
    // Sin petición que lo espere: el saludo `ready` o una publicación
    // espontánea (`state` con `id: null`, Fase 50).
    events.emit('message', { rendererId: renderer.id, message: msg });
  }

  /**
   * Arranca un renderer y espera su saludo `ready`. Rechaza si el proceso
   * muere o no saluda a tiempo.
   */
  function open(rendererId) {
    if (renderers.has(rendererId)) return Promise.reject(new Error(`El renderer ${rendererId} ya existe`));
    const child = spawnEngine(rendererId);
    const renderer = { id: rendererId, child, pending: new Map(), counter: 0, closing: false, alive: true };
    renderers.set(rendererId, renderer);

    const push = createLineSplitter({
      maxLineChars,
      onLine: (line) => handleLine(renderer, line),
      onOverflow: (chars) => events.emit('protocol-error', { rendererId, line: `línea de ${chars} caracteres descartada` }),
    });
    child.stdout.on('data', push);
    child.stderr?.on('data', (chunk) => events.emit('stderr', { rendererId, text: chunk.toString() }));

    const ready = new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`El renderer ${rendererId} no saludó en ${readyTimeoutMs} ms`)), readyTimeoutMs);
      const onMessage = ({ rendererId: id, message }) => {
        if (id !== rendererId || message.type !== 'ready') return;
        clearTimeout(timer);
        events.off('message', onMessage);
        resolve();
      };
      events.on('message', onMessage);
      renderer.rejectReady = (error) => {
        clearTimeout(timer);
        events.off('message', onMessage);
        reject(error);
      };
    });

    child.on('exit', (code, signal) => {
      renderer.alive = false;
      renderers.delete(rendererId);
      const detail = signal ? `señal ${signal}` : `código ${code}`;
      const error = new EngineCrashedError(rendererId, detail);
      renderer.rejectReady?.(error);
      fail(renderer, error);
      // Cerrarlo a propósito no es una caída.
      if (!renderer.closing) events.emit('crash', { rendererId, code, signal });
    });
    child.on('error', (err) => events.emit('protocol-error', { rendererId, line: err.message }));

    return ready;
  }

  /** Envía una petición al renderer `rendererId` y espera su respuesta. */
  function request(rendererId, payload) {
    const renderer = renderers.get(rendererId);
    if (!renderer || !renderer.alive) return Promise.reject(new Error(`No hay renderer ${rendererId}`));
    if (renderer.pending.size >= maxPending) return Promise.reject(new Error(`Demasiadas peticiones pendientes en ${rendererId}`));

    renderer.counter += 1;
    const id = `${rendererId}-${renderer.counter}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        renderer.pending.delete(id);
        reject(new Error(`Timeout esperando a ${rendererId} para '${payload.type}'`));
      }, requestTimeoutMs);
      renderer.pending.set(id, { resolve, reject, timer });
      try {
        renderer.child.stdin.write(`${JSON.stringify({ ...payload, id })}\n`);
      } catch (err) {
        clearTimeout(timer);
        renderer.pending.delete(id);
        reject(err);
      }
    });
  }

  /** Cierra un renderer de forma ordenada (`shutdown`) y, si no, lo mata. */
  async function close(rendererId, graceMs = 2000) {
    const renderer = renderers.get(rendererId);
    if (!renderer) return;
    renderer.closing = true;
    const exited = new Promise((resolve) => renderer.child.once('exit', resolve));
    try {
      renderer.child.stdin.write('{"type":"shutdown","id":"supervisor-close"}\n');
    } catch {
      // stdin ya cerrado: se mata abajo.
    }
    const timer = setTimeout(() => renderer.child.kill(), graceMs);
    await exited;
    clearTimeout(timer);
  }

  async function closeAll() {
    await Promise.all([...renderers.keys()].map((id) => close(id)));
  }

  return {
    open,
    request,
    close,
    closeAll,
    list: () => [...renderers.keys()],
    on: (event, listener) => events.on(event, listener),
  };
}

module.exports = { createEngineSupervisor, EngineCrashedError };
