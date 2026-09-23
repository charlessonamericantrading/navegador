// Cliente del canal de control de `engine_broker` (ADR 0001, etapa 2; Fase 67).
//
// El broker es el proceso que tiene la red, las cookies y el perfil. Este
// módulo lo arranca, espera su saludo (`ready` con el canal en `endpoint`),
// registra cada renderer para obtener su token de un solo uso y lo retira al
// cerrarlo. Nada más: la red y el almacenamiento van directamente de renderer
// a broker, sin pasar por aquí.
//
// Igual que `engine-supervisor.js`, recibe una función que lanza el proceso
// en vez de lanzarlo él: así los tests usan un broker falso sin compilar Rust.
const { createLineSplitter } = require('./engine-protocol');

// El canal de control son líneas cortas (un token son 64 caracteres).
const MAX_CONTROL_LINE_CHARS = 64 * 1024;

function createBrokerClient({ spawnBroker, readyTimeoutMs = 10_000, requestTimeoutMs = 10_000 }) {
  let child = null;
  let endpoint = null;
  let alive = false;
  let nextId = 1;
  const pending = new Map();
  const exitListeners = new Set();

  function rejectAll(error) {
    for (const entry of pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(error);
    }
    pending.clear();
  }

  /**
   * Arranca el broker y resuelve con `{ endpoint }` cuando saluda. Rechaza si
   * no saluda a tiempo o muere antes; en ese caso no deja el proceso vivo.
   */
  function start() {
    if (child) return Promise.reject(new Error('El broker ya está arrancado'));
    return new Promise((resolve, reject) => {
      let settled = false;
      const fail = (error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        try {
          child.kill();
        } catch {
          // Ya había terminado.
        }
        reject(error);
      };
      const timer = setTimeout(() => fail(new Error(`El broker no saludó en ${readyTimeoutMs} ms`)), readyTimeoutMs);

      child = spawnBroker();
      child.stdout.on('data', createLineSplitter({
        maxLineChars: MAX_CONTROL_LINE_CHARS,
        onOverflow: (chars) => console.error(`[Broker]: línea de control de ${chars} caracteres descartada`),
        onLine: (line) => {
          let message;
          try {
            message = JSON.parse(line);
          } catch {
            console.error('[Broker]: línea de control que no es JSON:', line.slice(0, 100));
            return;
          }
          if (!settled) {
            if (message.type === 'ready' && typeof message.endpoint === 'string' && message.endpoint) {
              settled = true;
              clearTimeout(timer);
              endpoint = message.endpoint;
              alive = true;
              resolve({ endpoint });
            }
            return;
          }
          const entry = pending.get(message.id);
          if (!entry) return;
          pending.delete(message.id);
          clearTimeout(entry.timer);
          if (message.type === 'error') entry.reject(new Error(`El broker rechazó la petición: ${message.message}`));
          else entry.resolve(message);
        },
      }));
      child.on('error', fail);
      child.on('exit', (code, signal) => {
        const wasAlive = alive;
        alive = false;
        fail(new Error(`El broker terminó antes de saludar (código ${code})`));
        rejectAll(new Error('El broker terminó'));
        if (wasAlive) for (const listener of exitListeners) listener({ code, signal });
      });
    });
  }

  function request(message) {
    if (!alive) return Promise.reject(new Error('El broker no está disponible'));
    return new Promise((resolve, reject) => {
      const id = `broker-${nextId++}`;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`El broker no contestó a '${message.type}' en ${requestTimeoutMs} ms`));
      }, requestTimeoutMs);
      pending.set(id, { resolve, reject, timer });
      try {
        child.stdin.write(`${JSON.stringify({ ...message, id })}\n`);
      } catch (error) {
        clearTimeout(timer);
        pending.delete(id);
        reject(error);
      }
    });
  }

  /** Da de alta un renderer y resuelve con su token (un solo uso). */
  async function register(renderer) {
    const reply = await request({ type: 'register', renderer });
    if (reply.type !== 'registered' || typeof reply.token !== 'string' || reply.token.length < 32) {
      throw new Error(`Respuesta inesperada del broker al registrar ${renderer}`);
    }
    return reply.token;
  }

  /** Retira un renderer: su token deja de valer y se le corta el canal. */
  async function revoke(renderer) {
    await request({ type: 'revoke', renderer });
  }

  /** Cierra el broker. No espera: al salir de la app no hay tiempo que perder. */
  function stop() {
    if (!child) return;
    alive = false;
    try {
      child.stdin.write('{"type":"shutdown","id":"quit"}\n');
    } catch {
      // Ya no escuchaba.
    }
    try {
      child.kill();
    } catch {
      // Ya había terminado.
    }
    rejectAll(new Error('El broker se cerró'));
  }

  /** Avisa si el broker muere después de haber saludado. */
  function onExit(listener) {
    exitListeners.add(listener);
    return () => exitListeners.delete(listener);
  }

  return {
    start,
    register,
    revoke,
    stop,
    onExit,
    get endpoint() {
      return endpoint;
    },
    get pid() {
      return child?.pid;
    },
  };
}

/**
 * El entorno de un renderer que usa este broker. Se parte del del proceso
 * principal y se le añaden solo las dos variables del canal.
 */
function rendererEnv(baseEnv, endpoint, token) {
  return { ...baseEnv, NAVEGADOR_IA_BROKER: endpoint, NAVEGADOR_IA_BROKER_TOKEN: token };
}

/**
 * Si el saludo de un renderer confirma que usa el broker. Lo dice el motor,
 * no quien lo arrancó: un motor antiguo, o uno que no recibiera el entorno,
 * saludaría con `local` (o sin el campo) y seguiría con su red y su disco.
 */
function usesRemoteBroker(message) {
  return message?.type === 'ready' && message.broker === 'remote';
}

module.exports = { createBrokerClient, rendererEnv, usesRemoteBroker };
