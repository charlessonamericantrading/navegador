// Corpus y medición base del motor (plan tarea 14, F25 y F39).
//
//   npm run bench                 -> 20 repeticiones por página
//   npm run bench -- --reps 5     -> más rápido, para probar el script
//   npm run bench -- --only a,b   -> solo esas páginas del corpus
//
// Sirve `engine/tests/corpus/` en local, arranca `engine_server` (release) con
// un perfil temporal y, por cada página del corpus:
//
// - navega `reps` veces en un proceso nuevo (la primera es la fría);
// - comprueba el ÉXITO antes de medir nada (plan 6.4.5): el título esperado,
//   esperando hasta 3 s a las páginas asíncronas; un fallo se guarda con su
//   motivo y no se descarta;
// - anota el tiempo hasta la respuesta de `navigate` y hasta el éxito, y la
//   memoria del proceso al terminar.
//
// Escribe todas las muestras en `docs/benchmarks/<fecha>-<commit>.json` junto
// con el entorno (commit, hash del binario, SO, CPU, memoria, versiones). No
// es una puerta: sale con 0 aunque haya fallos, porque medir no es aprobar.
// NO compara con Chrome: eso exige el protocolo completo de F39.
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

const rootDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const corpusDir = path.join(rootDir, 'engine', 'tests', 'corpus');
const repsArg = process.argv.indexOf('--reps');
const REPS = repsArg === -1 ? 20 : Number(process.argv[repsArg + 1]);
const SUCCESS_WAIT_MS = 3000;
const exeName = process.platform === 'win32' ? 'engine_server.exe' : 'engine_server';
const enginePath = path.join(rootDir, 'engine', 'target', 'release', exeName);

if (!Number.isInteger(REPS) || REPS < 1) {
  console.error('--reps debe ser un entero positivo');
  process.exit(2);
}
if (!fs.existsSync(enginePath)) {
  console.error(`No existe ${enginePath}. Compílalo antes: npm run build:engine`);
  process.exit(2);
}

const corpus = JSON.parse(fs.readFileSync(path.join(corpusDir, 'corpus.json'), 'utf8'));
// `--only id1,id2`: medir solo esas páginas (investigar un resultado concreto).
const onlyArg = process.argv.indexOf('--only');
if (onlyArg !== -1) {
  const ids = new Set(process.argv[onlyArg + 1].split(','));
  corpus.fixtures = corpus.fixtures.filter((f) => ids.has(f.id));
}
const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.json': 'application/json' };

// --- Servidor del corpus: solo ficheros de la carpeta, nada más ---
const server = http.createServer((req, res) => {
  const file = path.join(corpusDir, decodeURIComponent(new URL(req.url, 'http://x').pathname));
  if (path.relative(corpusDir, file).startsWith('..') || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { 'content-type': TYPES[path.extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
  fs.createReadStream(file).pipe(res);
}).listen(0, '127.0.0.1');
await new Promise((r) => server.on('listening', r));
const base = `http://127.0.0.1:${server.address().port}/`;

// --- Motor: un proceso por página, perfil temporal ---
function startEngine(profileDir) {
  const child = spawn(enginePath, [], { stdio: ['pipe', 'pipe', 'ignore'], env: { ...process.env, NAVEGADOR_IA_PROFILE_DIR: profileDir }, windowsHide: true });
  const waiting = new Map();
  let counter = 0;
  let onBoot;
  const booted = new Promise((r) => { onBoot = r; });
  readline.createInterface({ input: child.stdout, crlfDelay: Infinity }).on('line', (line) => {
    const msg = JSON.parse(line);
    if (msg.id === 'boot') onBoot();
    waiting.get(msg.id)?.(msg);
    waiting.delete(msg.id);
  });
  const request = (payload) => new Promise((resolve) => {
    const id = `b${++counter}`;
    waiting.set(id, resolve);
    child.stdin.write(`${JSON.stringify({ ...payload, id })}\n`);
  });
  return { child, booted, request };
}

function memoryBytes(pid) {
  try {
    if (process.platform === 'win32') {
      const out = execFileSync('powershell', ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).WorkingSet64`], { encoding: 'utf8' });
      return Number(out.trim());
    }
    const status = fs.readFileSync(`/proc/${pid}/status`, 'utf8');
    return Number(/VmRSS:\s+(\d+)/.exec(status)[1]) * 1024;
  } catch {
    return null;
  }
}

const quantile = (values, q) => {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)];
};
const round = (v) => (v === null ? null : Math.round(v * 10) / 10);

async function runFixture(fixture) {
  const profileDir = fs.mkdtempSync(path.join(os.tmpdir(), 'navegador-ia-bench-'));
  const engine = startEngine(profileDir);
  await engine.booted;
  await engine.request({ type: 'resize', width: 1280, height: 800 });
  const samples = [];
  for (let i = 0; i < REPS; i++) {
    const start = performance.now();
    const state = await engine.request({ type: 'navigate', url: base + fixture.ruta });
    const navigateMs = performance.now() - start;
    let title = state.title;
    let successMs = null;
    let reason = state.type === 'error' ? state.message : null;
    const matches = (t) => t === fixture.espera;
    const extraOk = (s) => !fixture.interactivosMinimos || (s.elements?.length ?? 0) >= fixture.interactivosMinimos;
    if (state.type === 'state' && matches(title) && extraOk(state)) {
      successMs = navigateMs;
    } else if (state.type === 'state') {
      // Páginas asíncronas: el título llega con un temporizador o una promesa.
      while (performance.now() - start < SUCCESS_WAIT_MS) {
        await new Promise((r) => setTimeout(r, 50));
        const s = await engine.request({ type: 'get_state' });
        title = s.title;
        if (matches(title) && extraOk(s)) {
          successMs = performance.now() - start;
          break;
        }
      }
      if (successMs === null) {
        reason = fixture.interactivosMinimos && matches(title)
          ? `menos de ${fixture.interactivosMinimos} elementos interactivos`
          : `título «${title}», se esperaba «${fixture.espera}»`;
      }
    }
    samples.push({ rep: i, navigateMs: round(navigateMs), successMs: round(successMs), ok: successMs !== null, title, reason });
  }
  const memory = memoryBytes(engine.child.pid);
  engine.child.kill();
  fs.rmSync(profileDir, { recursive: true, force: true });

  const ok = samples.filter((s) => s.ok);
  const warm = samples.slice(1).map((s) => s.navigateMs);
  return {
    id: fixture.id,
    categoria: fixture.categoria,
    aprobadas: ok.length,
    total: samples.length,
    fria: { navigateMs: samples[0].navigateMs, successMs: samples[0].successMs },
    caliente: { navigateMsP50: round(quantile(warm, 0.5)), navigateMsP95: round(quantile(warm, 0.95)), successMsP50: round(quantile(ok.slice(ok[0]?.rep === 0 ? 1 : 0).map((s) => s.successMs), 0.5)) },
    memoriaBytes: memory,
    motivosDeFallo: [...new Set(samples.filter((s) => !s.ok).map((s) => s.reason))],
    muestras: samples,
  };
}

function environment() {
  const git = (...args) => { try { return execFileSync('git', args, { cwd: rootDir, encoding: 'utf8' }).trim(); } catch { return null; } };
  const rustc = (() => { try { return execFileSync('rustc', ['--version'], { encoding: 'utf8' }).trim(); } catch { return null; } })();
  return {
    fecha: new Date().toISOString(),
    commit: git('rev-parse', 'HEAD'),
    arbolLimpio: git('status', '--porcelain') === '',
    binarioSha256: createHash('sha256').update(fs.readFileSync(enginePath)).digest('hex'),
    so: `${os.type()} ${os.release()} ${os.arch()}`,
    cpu: os.cpus()[0]?.model ?? null,
    nucleos: os.cpus().length,
    memoriaTotalBytes: os.totalmem(),
    node: process.version,
    rustc,
    repeticiones: REPS,
    notas: 'Un proceso de motor por página; la repetición 0 es la fría. Servidor local sin red externa. Sin comparación con Chrome.',
  };
}

const env = environment();
const results = [];
try {
  for (const fixture of corpus.fixtures) {
    process.stdout.write(`${fixture.id.padEnd(16)} `);
    const r = await runFixture(fixture);
    results.push(r);
    console.log(`${r.aprobadas}/${r.total}  fría ${r.fria.navigateMs} ms  p50 ${r.caliente.navigateMsP50} ms  p95 ${r.caliente.navigateMsP95} ms  ${r.memoriaBytes ? Math.round(r.memoriaBytes / 1048576) + ' MiB' : ''}${r.motivosDeFallo.length ? '  ✗ ' + r.motivosDeFallo.join(' | ') : ''}`);
  }
} finally {
  server.close();
}

const fullyPassing = results.filter((r) => r.aprobadas === r.total).length;
const report = { entorno: env, corpusVersion: corpus.version, resumen: { paginasQuePasanSiempre: fullyPassing, paginas: results.length }, resultados: results };
const outDir = path.join(rootDir, 'docs', 'benchmarks');
fs.mkdirSync(outDir, { recursive: true });
const outFile = path.join(outDir, `${env.fecha.slice(0, 10)}-${(env.commit ?? 'sin-git').slice(0, 7)}.json`);
fs.writeFileSync(outFile, `${JSON.stringify(report, null, 2)}\n`);
console.log(`\n${fullyPassing}/${results.length} páginas del corpus pasan en todas las repeticiones`);
console.log(`Informe: ${path.relative(rootDir, outFile)}`);
