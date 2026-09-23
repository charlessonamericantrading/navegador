// Frontera interfaz ↔ motor (plan F04: H05, H06, H21).
//
//   npm test
const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { validateEngineRequest, createLineSplitter, resolveAppPath } = require('../engine-protocol');

// --- validateEngineRequest (H05) ---

test('acepta las peticiones reales de la interfaz y devuelve una copia limpia', () => {
  assert.deepEqual(validateEngineRequest({ type: 'navigate', url: 'https://ejemplo.test/' }), { type: 'navigate', url: 'https://ejemplo.test/' });
  assert.deepEqual(validateEngineRequest({ type: 'type_text', x: 1.5, y: 2, text: 'hola', press_enter: false }), { type: 'type_text', x: 1.5, y: 2, text: 'hola', press_enter: false });
  assert.deepEqual(validateEngineRequest({ type: 'scroll', dx: 0, dy: -120 }), { type: 'scroll', dx: 0, dy: -120 });
  assert.deepEqual(validateEngineRequest({ type: 'new_tab' }), { type: 'new_tab' });
  assert.deepEqual(validateEngineRequest({ type: 'list_tabs' }), { type: 'list_tabs' });
});

test('el id del renderer se descarta: lo pone el proceso principal', () => {
  assert.deepEqual(validateEngineRequest({ type: 'ping', id: 'suplantado' }), { type: 'ping' });
});

test('shutdown no se puede pedir desde la página', () => {
  assert.throws(() => validateEngineRequest({ type: 'shutdown' }), /no permitido/);
});

test('rechaza tipos desconocidos, campos extra y lo que no es un objeto', () => {
  assert.throws(() => validateEngineRequest({ type: 'rm_rf' }), /no permitido/);
  assert.throws(() => validateEngineRequest({ type: 'ping', extra: 1 }), /campo no permitido/);
  assert.throws(() => validateEngineRequest({ type: '__proto__' }), /no permitido/);
  assert.throws(() => validateEngineRequest(null));
  assert.throws(() => validateEngineRequest([{ type: 'ping' }]));
  assert.throws(() => validateEngineRequest('ping'));
});

test('rechaza coordenadas no finitas y enteros fuera de rango', () => {
  assert.throws(() => validateEngineRequest({ type: 'click', x: NaN, y: 1 }), /finito/);
  assert.throws(() => validateEngineRequest({ type: 'click', x: Infinity, y: 1 }), /finito/);
  assert.throws(() => validateEngineRequest({ type: 'click', x: '10', y: 1 }), /finito/);
  assert.throws(() => validateEngineRequest({ type: 'resize', width: -1, height: 600 }), /entero/);
  assert.throws(() => validateEngineRequest({ type: 'resize', width: 1.5, height: 600 }), /entero/);
  assert.throws(() => validateEngineRequest({ type: 'switch_tab', tab_id: 2 ** 32 }), /entero/);
  assert.throws(() => validateEngineRequest({ type: 'scroll', dx: 0, dy: 2 ** 31 }), /entero/);
});

test('rechaza textos demasiado largos y tipos equivocados', () => {
  assert.throws(() => validateEngineRequest({ type: 'navigate', url: 'x'.repeat(8193) }), /supera/);
  assert.throws(() => validateEngineRequest({ type: 'press_key', key: 'x'.repeat(33) }), /supera/);
  assert.throws(() => validateEngineRequest({ type: 'type_text', x: 0, y: 0, text: 'a', press_enter: 'si' }), /booleano/);
  assert.throws(() => validateEngineRequest({ type: 'navigate' }), /texto/);
});

// --- createLineSplitter (H06) ---

function partir(opciones = {}) {
  const lineas = [];
  const desbordes = [];
  const push = createLineSplitter({ maxLineChars: opciones.max ?? 1000, onLine: (l) => lineas.push(l), onOverflow: (n) => desbordes.push(n) });
  return { push, lineas, desbordes };
}

test('un carácter UTF-8 partido entre dos trozos llega entero', () => {
  const { push, lineas } = partir();
  const bytes = Buffer.from('{"title":"España"}\n', 'utf8');
  const corte = bytes.indexOf(Buffer.from('ñ')) + 1; // a mitad de la ñ
  push(bytes.subarray(0, corte));
  push(bytes.subarray(corte));
  assert.deepEqual(lineas, ['{"title":"España"}']);
});

test('junta líneas partidas y separa varias en un mismo trozo', () => {
  const { push, lineas } = partir();
  push(Buffer.from('{"a":'));
  push(Buffer.from('1}\n{"b":2}\n{"c"'));
  push(Buffer.from(':3}\n'));
  assert.deepEqual(lineas, ['{"a":1}', '{"b":2}', '{"c":3}']);
});

test('una línea que supera el tope se descarta entera y la siguiente llega bien', () => {
  const { push, lineas, desbordes } = partir({ max: 10 });
  push(Buffer.from('x'.repeat(8)));
  push(Buffer.from('x'.repeat(8))); // ya son 16 sin salto: se descarta
  push(Buffer.from('xxxx\n{"ok":1}\n'));
  assert.deepEqual(lineas, ['{"ok":1}']);
  assert.equal(desbordes.length, 1);
});

test('una línea completa demasiado larga en un solo trozo también se descarta', () => {
  const { push, lineas, desbordes } = partir({ max: 5 });
  push(Buffer.from('123456789\nok\n'));
  assert.deepEqual(lineas, ['ok']);
  assert.deepEqual(desbordes, [9]);
});

// --- resolveAppPath (H21) ---

const RAIZ = path.resolve('/app/frontend/dist');

test('resuelve rutas normales dentro de la raíz', () => {
  assert.equal(resolveAppPath(RAIZ, 'app://./index.html'), path.join(RAIZ, 'index.html'));
  assert.equal(resolveAppPath(RAIZ, 'app://./assets/index-D51.js'), path.join(RAIZ, 'assets', 'index-D51.js'));
  assert.equal(resolveAppPath(RAIZ, 'app://./'), path.join(RAIZ, 'index.html'));
});

test('quita la consulta y el fragmento y decodifica la ruta', () => {
  assert.equal(resolveAppPath(RAIZ, 'app://./assets/a.js?v=3#x'), path.join(RAIZ, 'assets', 'a.js'));
  assert.equal(resolveAppPath(RAIZ, 'app://./img/mi%20logo.png'), path.join(RAIZ, 'img', 'mi logo.png'));
});

/** `true` si `resuelto` apunta fuera de la raíz: lo único que nunca puede pasar. */
function fuera(resuelto) {
  if (resuelto === null) return false;
  const rel = path.relative(RAIZ, resuelto);
  return rel === '' || rel === '..' || rel.startsWith(`..${path.sep}`) || path.isAbsolute(rel);
}

test('los ../ que el parser de URL entiende se normalizan y se quedan dentro', () => {
  // `%2e%2e` cuenta como `..` para el estándar WHATWG, igual que el literal.
  for (const url of ['app://./../main.js', 'app://./%2e%2e/%2e%2e/secreto', 'app://./../dist-malo/x.js']) {
    const resuelto = resolveAppPath(RAIZ, url);
    assert.ok(resuelto !== null && !fuera(resuelto), `${url} -> ${resuelto}`);
  }
});

test('lo que el parser de URL no normaliza se rechaza', () => {
  // Separadores codificados: al decodificar aparecen `../` que la URL no vio.
  const intentos = [
    'app://./..%2F..%2Fsecreto',
    'app://./..%5C..%5Csecreto',
    'app://./x/..%2F..%2F..%2Fsecreto',
    'app://./C:/Windows/win.ini',
    'app://./%00index.html',
    'app://./%E0%A4%A', // codificación rota
  ];
  for (const url of intentos) {
    assert.equal(resolveAppPath(RAIZ, url), null, url);
  }
});

test('un fichero cuyo nombre empieza por dos puntos no es una subida', () => {
  assert.equal(resolveAppPath(RAIZ, 'app://./..notas.txt'), path.join(RAIZ, '..notas.txt'));
});
