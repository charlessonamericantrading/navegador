// Frontera entre la interfaz y el motor Rust (plan F04, hallazgos H05, H06 y
// H21). Tres piezas puras, sin Electron, para poder probarlas con
// `node --test`:
//
// - `validateEngineRequest`: lo que el renderer puede pedir al motor. Lista
//   cerrada de tipos y campos; cualquier otra cosa se rechaza antes de llegar
//   al proceso Rust.
// - `createLineSplitter`: corta el stdout del motor en líneas NDJSON sin
//   romper caracteres UTF-8 partidos entre trozos y con un tope por línea.
// - `resolveAppPath`: traduce una URL `app://` a un fichero dentro de la raíz
//   de la interfaz, o `null` si se sale de ella.

const path = require('path');
const { StringDecoder } = require('string_decoder');

const MAX_URL = 8192;
const MAX_TEXT = 100_000;
const MAX_KEY = 32;

// Tipos de campo. Cada uno devuelve el valor ya comprobado o lanza.
const campo = {
  string: (max) => (v, nombre) => {
    if (typeof v !== 'string') throw new TypeError(`${nombre} debe ser texto`);
    if (v.length > max) throw new RangeError(`${nombre} supera ${max} caracteres`);
    return v;
  },
  optionalString: (max) => (v, nombre) => (v === undefined || v === null ? undefined : campo.string(max)(v, nombre)),
  bool: () => (v, nombre) => {
    if (typeof v !== 'boolean') throw new TypeError(`${nombre} debe ser booleano`);
    return v;
  },
  // Coordenadas: el motor las lee como f32. NaN o Infinity no son una
  // posición, y serde los rechazaría con un error menos claro.
  finite: () => (v, nombre) => {
    if (typeof v !== 'number' || !Number.isFinite(v)) throw new TypeError(`${nombre} debe ser un número finito`);
    return v;
  },
  integer: (min, max) => (v, nombre) => {
    if (!Number.isInteger(v) || v < min || v > max) throw new RangeError(`${nombre} debe ser un entero entre ${min} y ${max}`);
    return v;
  },
};

const U32 = [0, 0xffff_ffff];
const I32 = [-0x8000_0000, 0x7fff_ffff];

// Espejo de `EngineRequest` en `engine/crates/core/src/protocol.rs`, menos
// `shutdown`: apagar el motor es cosa del proceso principal al salir, no de la
// página. El `id` tampoco figura: lo pone siempre el proceso principal.
const ESQUEMA = {
  navigate: { url: campo.string(MAX_URL) },
  ping: {},
  resize: { width: campo.integer(...U32), height: campo.integer(...U32) },
  get_state: {},
  click: { x: campo.finite(), y: campo.finite() },
  scroll: { dx: campo.integer(...I32), dy: campo.integer(...I32) },
  type_text: { x: campo.finite(), y: campo.finite(), text: campo.string(MAX_TEXT), press_enter: campo.bool() },
  press_key: { key: campo.string(MAX_KEY) },
  back: {},
  forward: {},
  new_tab: { url: campo.optionalString(MAX_URL) },
  close_tab: { tab_id: campo.integer(...U32) },
  switch_tab: { tab_id: campo.integer(...U32) },
  list_tabs: {},
  get_accessibility_tree: {},
};

/**
 * Devuelve una copia limpia de `payload` con solo los campos del esquema, o
 * lanza con el motivo. Un campo desconocido también se rechaza: en la
 * frontera es preferible un error claro a que serde lo ignore en silencio.
 */
function validateEngineRequest(payload) {
  if (typeof payload !== 'object' || payload === null || Array.isArray(payload)) {
    throw new TypeError('la petición debe ser un objeto');
  }
  const { type, id: _idIgnorado, ...resto } = payload;
  if (typeof type !== 'string' || !Object.hasOwn(ESQUEMA, type)) {
    throw new TypeError(`tipo de petición no permitido: ${JSON.stringify(type)}`);
  }
  const esquema = ESQUEMA[type];
  for (const nombre of Object.keys(resto)) {
    if (!Object.hasOwn(esquema, nombre)) throw new TypeError(`campo no permitido en ${type}: ${nombre}`);
  }
  const limpio = { type };
  for (const [nombre, comprobar] of Object.entries(esquema)) {
    const valor = comprobar(resto[nombre], `${type}.${nombre}`);
    if (valor !== undefined) limpio[nombre] = valor;
  }
  return limpio;
}

/**
 * Corta un flujo de trozos en líneas. `StringDecoder` guarda los bytes de un
 * carácter UTF-8 partido hasta que llega el resto; `chunk.toString()` por
 * trozo los convertía en caracteres de reemplazo.
 *
 * Una línea que supera `maxLineChars` se descarta entera (hasta el siguiente
 * salto) y se avisa por `onOverflow`: sin tope, un motor que no termina una
 * línea haría crecer la memoria del proceso principal sin límite.
 */
function createLineSplitter({ maxLineChars, onLine, onOverflow }) {
  const decoder = new StringDecoder('utf8');
  let buffer = '';
  let descartando = false;

  return function push(chunk) {
    let texto = decoder.write(chunk);
    while (texto.length > 0) {
      const salto = texto.indexOf('\n');
      if (salto === -1) {
        if (!descartando) {
          buffer += texto;
          if (buffer.length > maxLineChars) {
            onOverflow(buffer.length);
            buffer = '';
            descartando = true;
          }
        }
        return;
      }
      const trozo = texto.slice(0, salto);
      texto = texto.slice(salto + 1);
      if (descartando) {
        descartando = false;
        continue;
      }
      const linea = buffer + trozo;
      buffer = '';
      if (linea.length > maxLineChars) {
        onOverflow(linea.length);
        continue;
      }
      onLine(linea);
    }
  };
}

/**
 * Traduce `app://./ruta?consulta` a un fichero dentro de `baseDir`, o `null`
 * si la ruta se sale. La comprobación anterior (`startsWith(baseDir)`) daba
 * por buena una carpeta hermana (`dist-malo`), no quitaba la consulta ni
 * decodificaba `%20`. Aquí se parsea como URL y se compara con
 * `path.relative`, que es la pregunta correcta: ¿hay que subir para llegar?
 */
function resolveAppPath(baseDir, requestUrl) {
  let pathname;
  try {
    pathname = decodeURIComponent(new URL(requestUrl).pathname);
  } catch {
    return null;
  }
  if (pathname.includes('\0')) return null;
  const relativa = pathname.replace(/^\/+/, '') || 'index.html';
  const raiz = path.resolve(baseDir);
  const destino = path.resolve(raiz, relativa);
  const desdeRaiz = path.relative(raiz, destino);
  const subeFuera = desdeRaiz === '..' || desdeRaiz.startsWith(`..${path.sep}`);
  if (desdeRaiz === '' || subeFuera || path.isAbsolute(desdeRaiz)) return null;
  return destino;
}

module.exports = { validateEngineRequest, createLineSplitter, resolveAppPath, ESQUEMA };
