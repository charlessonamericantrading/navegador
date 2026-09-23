// Prueba de humo de la aplicación EMPAQUETADA (plan F01/F38, hallazgo H19).
//
//   npm run smoke                       -> dist/win-unpacked (o el de la plataforma)
//   node scripts/smoke-packaged.mjs <ejecutable>
//
// El CI empaquetaba con la carpeta del motor vacía: el paquete se generaba y
// nadie comprobaba que funcionara. Esto arranca el ejecutable real con el
// protocolo de DevTools en un puerto local y comprueba desde fuera, igual que
// lo haría una persona, que:
//
//   1. la interfaz carga por `app://` y React monta;
//   2. el motor Rust empaquetado responde `pong`;
//   3. navega de verdad a una página local y devuelve su título y su captura;
//   4. la frontera IPC rechaza una petición fuera del esquema (Fase 54);
//   5. los efectos de conexión no se re-ejecutan en bucle (Fase 55).
//
// Sale con 0 si todo pasa y con 1 en cuanto algo falla, con el motivo. Cierra
// el árbol de procesos de la aplicación al terminar, pase lo que pase.
import { spawn, execFileSync } from 'node:child_process';
import http from 'node:http';
import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DEFAULT_EXE = {
  win32: 'dist/win-unpacked/Navegador IA.exe',
  linux: 'dist/linux-unpacked/navegador-ia-desktop',
  darwin: 'dist/mac/Navegador IA.app/Contents/MacOS/Navegador IA',
}[process.platform];
const exe = path.resolve(desktopDir, process.argv[2] ?? DEFAULT_EXE);
const PORT = 9335;
const TOTAL_TIMEOUT_MS = 120_000;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const checks = [];
function check(name, ok, detail) {
  checks.push({ name, ok, detail });
  console.log(`${ok ? 'OK  ' : 'FAIL'} ${name}${detail === undefined ? '' : ` — ${typeof detail === 'string' ? detail : JSON.stringify(detail)}`}`);
}

function killTree(pid) {
  if (!pid) return;
  try {
    if (process.platform === 'win32') execFileSync('taskkill', ['/F', '/T', '/PID', String(pid)], { stdio: 'ignore' });
    else process.kill(-pid, 'SIGKILL');
  } catch {
    // Ya había terminado.
  }
}

if (!fs.existsSync(exe)) {
  console.error(`FAIL no existe el ejecutable empaquetado: ${exe}`);
  process.exit(1);
}

// Página que el motor tiene que descargar y pintar: título conocido y texto.
const server = http.createServer((_req, res) => {
  res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
  res.end('<!doctype html><html><head><title>Humo empaquetado</title></head><body><h1>Página de humo</h1><p>ñandú</p></body></html>');
}).listen(0, '127.0.0.1');
await new Promise((r) => server.on('listening', r));
const pageUrl = `http://127.0.0.1:${server.address().port}/`;

const app = spawn(exe, [`--remote-debugging-port=${PORT}`], { stdio: 'ignore', detached: process.platform !== 'win32' });
const watchdog = setTimeout(() => {
  console.error(`FAIL la prueba superó ${TOTAL_TIMEOUT_MS} ms`);
  killTree(app.pid);
  process.exit(1);
}, TOTAL_TIMEOUT_MS);

let ws;
try {
  let target;
  for (let i = 0; i < 60 && !target; i++) {
    await sleep(500);
    try {
      target = (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).find((t) => t.type === 'page');
    } catch {
      // DevTools todavía no escucha.
    }
  }
  if (!target) throw new Error('la ventana no llegó a abrirse (sin página en DevTools)');

  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve);
    ws.addEventListener('error', reject);
  });
  let nextId = 0;
  const pending = new Map();
  ws.addEventListener('message', (e) => {
    const msg = JSON.parse(e.data);
    pending.get(msg.id)?.(msg);
  });
  const evaluate = (expression) => new Promise((resolve) => {
    const id = ++nextId;
    pending.set(id, (msg) => resolve(msg.result?.result?.value ?? { excepcion: msg.result?.exceptionDetails?.exception?.description }));
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
  });
  const engine = (payload) => evaluate(`window.electronAPI.sendEngineRequest(${JSON.stringify(payload)}).then(r => r, e => ({ rechazada: String(e && e.message || e) }))`);

  // 1. Interfaz.
  let ui;
  for (let i = 0; i < 20; i++) {
    ui = await evaluate(`({ url: location.href, montada: (document.getElementById('root')?.children.length ?? 0) > 0, ipc: typeof window.electronAPI?.sendEngineRequest })`);
    if (ui?.montada) break;
    await sleep(500);
  }
  check('la interfaz carga por app:// y React monta', ui?.url?.startsWith('app://') && ui?.montada && ui?.ipc === 'function', ui);

  // 2. Motor empaquetado.
  const pong = await engine({ type: 'ping' });
  check('el motor empaquetado responde pong', pong?.type === 'pong', pong);

  // 3. Navegación real.
  const state = await engine({ type: 'navigate', url: pageUrl });
  check('navega a una página local y devuelve su título', state?.type === 'state' && state.title === 'Humo empaquetado', { type: state?.type, title: state?.title, rechazada: state?.rechazada });
  check('la respuesta trae una captura PNG', typeof state?.screenshot === 'string' && state.screenshot.startsWith('iVBORw0KGgo'), state?.screenshot?.slice(0, 12));

  // 4. Frontera IPC.
  const rechazo = await engine({ type: 'shutdown' });
  const sigue = await engine({ type: 'ping' });
  check('la IPC rechaza shutdown desde la página y el motor sigue vivo', typeof rechazo?.rechazada === 'string' && sigue?.type === 'pong', { rechazo, sigue: sigue?.type });

  // 5. Sin bucle de reconexión. Tres pings separados 2 s: un bucle genera
  //    peticiones SIN PARAR, así que el último intervalo tampoco estaría en
  //    calma. Una petición puntual de la interfaz (un `resize` al asentarse la
  //    ventana, como pasó en el runner de GitHub) solo afecta al primero.
  //    Antes se exigía «exactamente +1» en un único intervalo, y eso daba
  //    falsos fallos.
  const idDe = (r) => Number(r?.id?.split('-')[1]);
  const p0 = idDe(await engine({ type: 'ping' }));
  await sleep(2000);
  const p1 = idDe(await engine({ type: 'ping' }));
  await sleep(2000);
  const p2 = idDe(await engine({ type: 'ping' }));
  check('los efectos de conexión no se re-ejecutan en bucle', p2 - p1 === 1 && p1 - p0 <= 3, { saltos: [p1 - p0, p2 - p1] });
} catch (err) {
  check('la prueba llegó al final', false, err instanceof Error ? err.message : String(err));
} finally {
  clearTimeout(watchdog);
  ws?.close();
  killTree(app.pid);
  server.close();
}

const failed = checks.filter((c) => !c.ok);
console.log(`\n${checks.length - failed.length}/${checks.length} comprobaciones superadas`);
process.exit(failed.length === 0 ? 0 : 1);
