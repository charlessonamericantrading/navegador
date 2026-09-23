// Cliente del broker (Fase 67): arranque, registro, fallos y, si están
// compilados, los binarios reales de broker y motor juntos.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const http = require('node:http');
const path = require('node:path');
const { createBrokerClient, rendererEnv, usesRemoteBroker } = require('../engine-broker');
const { createLineSplitter } = require('../engine-protocol');

const FAKE = path.join(__dirname, 'fixtures', 'fake-broker.js');
const falso = (mode) => () => spawn(process.execPath, [FAKE], { env: { ...process.env, FAKE_BROKER_MODE: mode }, stdio: ['pipe', 'pipe', 'inherit'] });

test('arranca, registra y retira contra el canal de control', async () => {
  const broker = createBrokerClient({ spawnBroker: falso('normal') });
  const { endpoint } = await broker.start();
  assert.equal(endpoint, 'canal-falso');
  assert.equal(await broker.register('tab-1'), 'f'.repeat(64));
  await broker.revoke('tab-1');
  broker.stop();
});

test('un broker que no saluda se rechaza a tiempo y no queda vivo', async () => {
  const broker = createBrokerClient({ spawnBroker: falso('mudo'), readyTimeoutMs: 300 });
  await assert.rejects(broker.start(), /no saludó en 300 ms/);
  const pid = broker.pid;
  await new Promise((r) => setTimeout(r, 200));
  assert.throws(() => process.kill(pid, 0), 'el proceso mudo debe haberse matado');
});

test('si el broker muere tras saludar, las peticiones fallan y se avisa', async () => {
  const broker = createBrokerClient({ spawnBroker: falso('muere') });
  await broker.start();
  const aviso = new Promise((resolve) => broker.onExit(resolve));
  await assert.rejects(broker.register('tab-1'), /El broker terminó/);
  assert.equal((await aviso).code, 4);
  await assert.rejects(broker.register('tab-2'), /no está disponible/);
});

test('un error del broker llega como rechazo con su motivo', async () => {
  const broker = createBrokerClient({ spawnBroker: falso('rechaza') });
  await broker.start();
  await assert.rejects(broker.register('tab-1'), /no me da la gana/);
  broker.stop();
});

test('solo un saludo con broker remoto cuenta como remoto', () => {
  assert.equal(usesRemoteBroker({ type: 'ready', broker: 'remote' }), true);
  assert.equal(usesRemoteBroker({ type: 'ready', broker: 'local' }), false);
  assert.equal(usesRemoteBroker({ type: 'ready' }), false, 'un motor antiguo no dice nada y no vale');
  assert.deepEqual(rendererEnv({ PATH: 'x' }, 'e', 't'), { PATH: 'x', NAVEGADOR_IA_BROKER: 'e', NAVEGADOR_IA_BROKER_TOKEN: 't' });
});

// Los dos binarios del mismo perfil de compilación: un broker de una rama y un
// motor de otra no tienen por qué hablar el mismo protocolo.
const exe = (name) => (process.platform === 'win32' ? `${name}.exe` : name);
const binarios = ['release', 'debug']
  .map((perfil) => path.join(__dirname, '..', '..', 'engine', 'target', perfil))
  .find((dir) => fs.existsSync(path.join(dir, exe('engine_broker'))) && fs.existsSync(path.join(dir, exe('engine_server'))));

test('con los binarios reales, el motor usa el broker y deja de poder navegar al retirarlo', { skip: binarios ? false : 'engine_broker/engine_server no están compilados (npm run build:engine)' }, async (t) => {
  const perfil = fs.mkdtempSync(path.join(os.tmpdir(), 'navegador-ia-broker-'));
  const perfilMotor = path.join(perfil, 'motor');
  const env = { ...process.env, NAVEGADOR_IA_PROFILE_DIR: path.join(perfil, 'broker') };
  const broker = createBrokerClient({ spawnBroker: () => spawn(path.join(binarios, exe('engine_broker')), [], { env, stdio: ['pipe', 'pipe', 'inherit'] }) });
  const { endpoint } = await broker.start();
  const token = await broker.register('tab-1');

  const server = http.createServer((_req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<!doctype html><title>t</title><script>localStorage.setItem("k","v"); document.title = localStorage.getItem("k");</script>');
  }).listen(0, '127.0.0.1');
  await new Promise((r) => server.on('listening', r));

  const motor = spawn(path.join(binarios, exe('engine_server')), [], {
    env: rendererEnv({ ...process.env, NAVEGADOR_IA_PROFILE_DIR: perfilMotor }, endpoint, token),
    stdio: ['pipe', 'pipe', 'ignore'],
  });
  t.after(() => {
    motor.kill();
    broker.stop();
    server.close();
    fs.rmSync(perfil, { recursive: true, force: true });
  });
  const lineas = [];
  let avisar = null;
  motor.stdout.on('data', createLineSplitter({ maxLineChars: 64 * 1024 * 1024, onOverflow: () => {}, onLine: (l) => { lineas.push(JSON.parse(l)); avisar?.(); } }));
  const siguiente = async (id) => {
    for (;;) {
      const i = lineas.findIndex((m) => (id === undefined ? m.type === 'ready' : m.id === id));
      if (i >= 0) return lineas.splice(i, 1)[0];
      await new Promise((r) => { avisar = r; });
    }
  };
  const pedir = (msg) => {
    motor.stdin.write(`${JSON.stringify(msg)}\n`);
    return siguiente(msg.id);
  };

  assert.ok(usesRemoteBroker(await siguiente()), 'el motor debe saludar con broker remoto');
  const estado = await pedir({ id: '1', type: 'navigate', url: `http://127.0.0.1:${server.address().port}/` });
  assert.equal(estado.title, 'v', 'el localStorage funciona a través del broker');
  assert.equal(fs.existsSync(perfilMotor), false, 'el motor no crea su perfil');

  await broker.revoke('tab-1');
  const tras = await pedir({ id: '2', type: 'navigate', url: `http://127.0.0.1:${server.address().port}/` });
  assert.equal(tras.type, 'error');
  assert.match(tras.message, /broker/);
});
