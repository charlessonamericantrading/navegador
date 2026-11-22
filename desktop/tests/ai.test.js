// Credenciales y proveedor de IA del proceso principal (plan F05, H04), sin
// arrancar Electron: `safeStorage`, el disco y `fetch` se inyectan.
//
//   npm test
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { createCredentialStore } = require('../ai-credentials');
const { createGeminiProvider, redact, geminiUrl } = require('../ai-provider');
const { createModelSettings, isValidModelName, DEFAULT_GEMINI_MODEL } = require('../ai-settings');

/** `safeStorage` falso: "cifra" invirtiendo, suficiente para ver que no va en claro. */
function safeStorageFalso({ disponible = true, backend } = {}) {
  return {
    isEncryptionAvailable: () => disponible,
    ...(backend ? { getSelectedStorageBackend: () => backend } : {}),
    encryptString: (texto) => Buffer.from([...texto].reverse().join(''), 'utf8'),
    decryptString: (buffer) => [...buffer.toString('utf8')].reverse().join(''),
  };
}

function almacen(t, opciones = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ai-cred-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const filePath = path.join(dir, 'ai-credentials.json');
  const store = createCredentialStore({ safeStorage: safeStorageFalso(opciones), filePath, fs, platform: opciones.platform ?? 'win32' });
  return { store, filePath };
}

const CLAVE = 'AIzaSy-clave-de-prueba';

test('la clave se guarda cifrada y el estado no la revela', (t) => {
  const { store, filePath } = almacen(t);

  const estado = store.set(`  ${CLAVE}  `);

  assert.deepEqual(estado, { configured: true, persisted: true, secureStorage: true });
  assert.ok(!fs.readFileSync(filePath, 'utf8').includes(CLAVE), 'en disco no puede estar en claro');
  assert.ok(!JSON.stringify(estado).includes(CLAVE));
  assert.equal(store.get(), CLAVE, 'el proceso principal sí la recupera, sin espacios');
});

test('sin almacén seguro la clave solo vive en memoria', (t) => {
  const { store, filePath } = almacen(t, { platform: 'linux', backend: 'basic_text' });

  const estado = store.set(CLAVE);

  assert.deepEqual(estado, { configured: true, persisted: false, secureStorage: false });
  assert.equal(fs.existsSync(filePath), false, 'basic_text no es cifrado real: no se escribe nada');
  assert.equal(store.get(), CLAVE);
});

test('borrar elimina la clave de disco y de memoria', (t) => {
  const { store, filePath } = almacen(t);
  store.set(CLAVE);

  assert.deepEqual(store.clear(), { configured: false, persisted: false, secureStorage: true });
  assert.equal(fs.existsSync(filePath), false);
  assert.equal(store.get(), null);
});

test('rechaza claves vacías o que no son texto', (t) => {
  const { store } = almacen(t);
  assert.throws(() => store.set('   '));
  assert.throws(() => store.set(42));
  assert.throws(() => store.set('x'.repeat(513)));
});

test('un fichero ilegible o de otra máquina no es una clave', (t) => {
  const { store, filePath } = almacen(t);
  fs.writeFileSync(filePath, 'no es json');
  assert.equal(store.get(), null);
  assert.equal(store.status().configured, false);
});

/** `fetch` falso que apunta lo que recibe y responde con `respuesta`. */
function fetchFalso(respuesta) {
  const llamadas = [];
  const fetch = async (url, init) => {
    llamadas.push({ url, init });
    return typeof respuesta === 'function' ? respuesta(init) : respuesta;
  };
  return { fetch, llamadas };
}

const RESPUESTA_OK = () => new Response(JSON.stringify({ candidates: [{ content: { parts: [{ text: '{"action":"finish"}' }] } }] }), { status: 200 });

test('la clave viaja en cabecera, nunca en la URL', async () => {
  const { fetch, llamadas } = fetchFalso(RESPUESTA_OK);
  const provider = createGeminiProvider({ getKey: () => CLAVE, fetch });

  assert.equal(await provider.generate('r1', 'prompt'), '{"action":"finish"}');
  assert.ok(!llamadas[0].url.includes(CLAVE));
  assert.equal(llamadas[0].init.headers['x-goog-api-key'], CLAVE);
});

test('un error del proveedor no devuelve la clave al renderer', async () => {
  const { fetch } = fetchFalso(() => new Response(`clave rechazada: ${CLAVE}`, { status: 400 }));
  const provider = createGeminiProvider({ getKey: () => CLAVE, fetch });

  await assert.rejects(provider.generate('r1', 'prompt'), (err) => {
    assert.ok(!err.message.includes(CLAVE), err.message);
    assert.match(err.message, /API error \(400\)/);
    return true;
  });
});

test('cancelar por requestId aborta la petición HTTP', async () => {
  const { fetch } = fetchFalso((init) => new Promise((_resolve, reject) => {
    init.signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
  }));
  const provider = createGeminiProvider({ getKey: () => CLAVE, fetch });

  const peticion = provider.generate('r1', 'prompt');
  provider.cancel('r1');

  await assert.rejects(peticion, /cancelado/);
});

test('sin clave configurada falla sin llamar al proveedor', async () => {
  const { fetch, llamadas } = fetchFalso(RESPUESTA_OK);
  const provider = createGeminiProvider({ getKey: () => null, fetch });

  await assert.rejects(provider.generate('r1', 'prompt'), /No hay clave/);
  assert.equal(llamadas.length, 0);
});

test('valida lo que llega por IPC', async () => {
  const { fetch } = fetchFalso(RESPUESTA_OK);
  const provider = createGeminiProvider({ getKey: () => CLAVE, fetch });

  await assert.rejects(provider.generate('', 'prompt'));
  await assert.rejects(provider.generate('r1', 42));
  await assert.rejects(provider.generate('r1', 'x'.repeat(200_001)), /demasiado largo/);
});

test('redact oculta todas las apariciones', () => {
  assert.equal(redact(`a ${CLAVE} b ${CLAVE}`, CLAVE), 'a [clave oculta] b [clave oculta]');
});

// --- Modelo configurable (tarea 3.4) ---

test('el modelo por defecto no es el retirado y va en la URL de la petición', async () => {
  assert.ok(!DEFAULT_GEMINI_MODEL.includes('2.0'), 'gemini-2.0-flash se retiró el 1-jun-2026');
  const { fetch, llamadas } = fetchFalso(RESPUESTA_OK);
  const provider = createGeminiProvider({ getKey: () => CLAVE, fetch });

  await provider.generate('r1', 'prompt');
  assert.equal(llamadas[0].url, `https://generativelanguage.googleapis.com/v1beta/models/${DEFAULT_GEMINI_MODEL}:generateContent`);
});

test('el proveedor usa el modelo que se le configura, y lo relee en cada petición', async () => {
  const { fetch, llamadas } = fetchFalso(RESPUESTA_OK);
  let modelo = 'gemini-3.1-flash-lite';
  const provider = createGeminiProvider({ getKey: () => CLAVE, getModel: () => modelo, fetch });

  await provider.generate('r1', 'a');
  modelo = 'gemini-3.8-flash';
  await provider.generate('r2', 'b');
  assert.match(llamadas[0].url, /models\/gemini-3\.1-flash-lite:generateContent$/);
  assert.match(llamadas[1].url, /models\/gemini-3\.8-flash:generateContent$/);
});

test('un nombre de modelo que no es un nombre no puede cambiar el destino de la petición', async () => {
  for (const malo of ['', 'x', 'GEMINI', 'gemini/../../v1/otra', 'gemini?key=1', 'gemini:otra', 'gemini 3', 'a'.repeat(80), null, 7]) {
    assert.equal(isValidModelName(malo), false, String(malo));
    assert.throws(() => geminiUrl(malo), /no válido/);
  }
  assert.equal(isValidModelName('gemini-3.5-flash'), true);

  const { fetch, llamadas } = fetchFalso(RESPUESTA_OK);
  const provider = createGeminiProvider({ getKey: () => CLAVE, getModel: () => '../../evil', fetch });
  await assert.rejects(provider.generate('r1', 'prompt'), /no válido/);
  assert.equal(llamadas.length, 0, 'ni se llama al proveedor');
});

function ajustes(t, env = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ai-set-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const filePath = path.join(dir, 'ai-settings.json');
  return { settings: createModelSettings({ filePath, fs, env }), filePath };
}

test('el ajuste del modelo se guarda, se lee y se restablece', (t) => {
  const { settings } = ajustes(t);
  assert.deepEqual(settings.info(), { model: DEFAULT_GEMINI_MODEL, defaultModel: DEFAULT_GEMINI_MODEL });

  assert.equal(settings.set('  gemini-3.8-flash  ').model, 'gemini-3.8-flash');
  assert.equal(settings.get(), 'gemini-3.8-flash');

  assert.equal(settings.reset().model, DEFAULT_GEMINI_MODEL);
  assert.equal(settings.reset().model, DEFAULT_GEMINI_MODEL, 'restablecer sin nada guardado no falla');
});

test('guardar un nombre no válido se rechaza y no toca lo guardado', (t) => {
  const { settings } = ajustes(t);
  settings.set('gemini-3.8-flash');
  assert.throws(() => settings.set('mal nombre'), /no válido/);
  assert.equal(settings.get(), 'gemini-3.8-flash');
});

test('la variable de entorno manda sobre el ajuste guardado, y un fichero estropeado no rompe nada', (t) => {
  const { settings, filePath } = ajustes(t, { NAVEGADOR_GEMINI_MODEL: 'gemini-3.1-flash-lite' });
  settings.set('gemini-3.8-flash');
  assert.equal(settings.get(), 'gemini-3.1-flash-lite');

  const { settings: otro, filePath: otraRuta } = ajustes(t);
  fs.writeFileSync(otraRuta, '{esto no es json', 'utf8');
  assert.equal(otro.get(), DEFAULT_GEMINI_MODEL);
  assert.ok(filePath);
});
