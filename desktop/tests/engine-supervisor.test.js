// Supervisor de renderers (ADR 0001, etapa 1): un proceso por renderer y
// caídas contenidas.
//
//   npm test
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const { createEngineSupervisor, EngineCrashedError } = require('../engine-supervisor');

/** Supervisor con arranque inyectado que guarda cada proceso para poder matarlo. */
function supervisorCon(comando) {
  const hijos = new Map();
  const supervisor = createEngineSupervisor({
    spawnEngine: (id) => {
      const [exe, ...args] = comando;
      const hijo = spawn(exe, args, { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
      hijos.set(id, hijo);
      return hijo;
    },
    requestTimeoutMs: 20_000,
  });
  const caidas = [];
  supervisor.on('crash', (c) => caidas.push(c));
  return { supervisor, hijos, caidas };
}

const MOTOR_FALSO = [process.execPath, path.join(__dirname, 'fixtures', 'fake-engine.js')];

test('cada renderer es un proceso distinto y responde por separado', async (t) => {
  const { supervisor, hijos } = supervisorCon(MOTOR_FALSO);
  t.after(() => supervisor.closeAll());

  await supervisor.open('a');
  await supervisor.open('b');
  assert.notEqual(hijos.get('a').pid, hijos.get('b').pid);
  assert.equal((await supervisor.request('a', { type: 'ping' })).id, 'a-1');
  assert.equal((await supervisor.request('b', { type: 'ping' })).id, 'b-1');
  assert.deepEqual(supervisor.list().sort(), ['a', 'b']);
});

test('una caída rechaza las peticiones pendientes de ESE renderer al momento y no toca al otro', async (t) => {
  const { supervisor, caidas } = supervisorCon(MOTOR_FALSO);
  t.after(() => supervisor.closeAll());
  await supervisor.open('a');
  await supervisor.open('b');

  const colgada = supervisor.request('a', { type: 'hang' });
  const inicio = Date.now();
  supervisor.request('a', { type: 'crash' }).catch(() => {});

  await assert.rejects(colgada, EngineCrashedError);
  assert.ok(Date.now() - inicio < 5000, 'se rechaza por la caída, no por agotar el plazo');
  assert.deepEqual(caidas.map((c) => [c.rendererId, c.code]), [['a', 3]]);

  assert.equal((await supervisor.request('b', { type: 'ping' })).type, 'pong');
  assert.deepEqual(supervisor.list(), ['b']);
  await assert.rejects(supervisor.request('a', { type: 'ping' }), /No hay renderer a/);
});

test('cerrar a propósito no se cuenta como caída', async (t) => {
  const { supervisor, caidas } = supervisorCon(MOTOR_FALSO);
  t.after(() => supervisor.closeAll());
  await supervisor.open('a');

  await supervisor.close('a');

  assert.deepEqual(caidas, []);
  assert.deepEqual(supervisor.list(), []);
});

test('un renderer que muere antes de saludar hace fallar open', async () => {
  const { supervisor } = supervisorCon([process.execPath, '-e', 'process.exit(7)']);
  await assert.rejects(supervisor.open('a'), EngineCrashedError);
});

test('no se pueden abrir dos renderers con el mismo id', async (t) => {
  const { supervisor } = supervisorCon(MOTOR_FALSO);
  t.after(() => supervisor.closeAll());
  await supervisor.open('a');
  await assert.rejects(supervisor.open('a'), /ya existe/);
});

// --- Experimento de aceptación del ADR contra el motor real ---

const motorReal = ['release', 'debug']
  .map((perfil) => path.join(__dirname, '..', '..', 'engine', 'target', perfil, process.platform === 'win32' ? 'engine_server.exe' : 'engine_server'))
  .find((p) => fs.existsSync(p));

test('ADR 0001: con el motor real, matar un renderer a mitad de una navegación no afecta al otro', { skip: motorReal ? false : 'engine_server no está compilado (npm run build:engine)' }, async (t) => {
  // Una ruta que tarda en responder (para tener una petición pendiente al
  // matar) y otra inmediata.
  const servidor = http.createServer((req, res) => {
    const responder = () => {
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      res.end(`<!doctype html><title>${req.url === '/lenta' ? 'Lenta' : 'Rápida'}</title><p>hola</p>`);
    };
    if (req.url === '/lenta') setTimeout(responder, 10_000);
    else responder();
  }).listen(0, '127.0.0.1');
  await new Promise((r) => servidor.on('listening', r));
  const base = `http://127.0.0.1:${servidor.address().port}`;

  const { supervisor, hijos, caidas } = supervisorCon([motorReal]);
  t.after(async () => {
    await supervisor.closeAll();
    servidor.closeAllConnections();
    servidor.close();
  });

  await supervisor.open('pestana-1');
  await supervisor.open('pestana-2');

  const pendiente = supervisor.request('pestana-1', { type: 'navigate', url: `${base}/lenta` });
  await new Promise((r) => setTimeout(r, 300));
  hijos.get('pestana-1').kill();

  await assert.rejects(pendiente, EngineCrashedError);
  assert.equal(caidas.length, 1);
  assert.equal(caidas[0].rendererId, 'pestana-1');

  assert.equal((await supervisor.request('pestana-2', { type: 'ping' })).type, 'pong');
  const estado = await supervisor.request('pestana-2', { type: 'navigate', url: `${base}/rapida` });
  assert.equal(estado.type, 'state');
  assert.equal(estado.title, 'Rápida');
});
