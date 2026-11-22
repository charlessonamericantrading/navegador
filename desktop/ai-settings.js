// Modelo de IA configurable (tarea 3.4 del plan). El modelo que se usaba
// (`gemini-2.0-flash`) lo retiro Google el 1 de junio de 2026, y los nombres de
// sustitucion cambian cada pocos meses: dejarlo escrito en el codigo obliga a
// publicar una version nueva cada vez. Aqui el modelo es un ajuste.
//
// Prioridad: variable de entorno `NAVEGADOR_GEMINI_MODEL` > ajuste guardado >
// valor por defecto. Fabrica con dependencias inyectadas para probarla con
// `node --test` sin Electron.

// Sustituto de `gemini-2.0-flash` segun las notas de version de Google. Las
// paginas oficiales no coinciden entre si (hablan de 3.5, 3.6 y 3.8), asi que
// este valor hay que confirmarlo en https://ai.google.dev/gemini-api/docs/deprecations
// y, si ha cambiado, basta con ponerlo en Ajustes.
const DEFAULT_GEMINI_MODEL = 'gemini-3.5-flash';

// Lo que va a parar a la URL: solo letras, cifras, punto y guion. Cualquier otra
// cosa (barras, `?`, `:`) permitiria que el ajuste cambiara el destino de la
// peticion.
const MODEL_NAME = /^[a-z][a-z0-9.-]{1,63}$/;

function isValidModelName(name) {
  return typeof name === 'string' && MODEL_NAME.test(name);
}

function createModelSettings({ filePath, fs, env = {} }) {
  function readSaved() {
    try {
      const data = JSON.parse(fs.readFileSync(filePath, 'utf8'));
      return isValidModelName(data.geminiModel) ? data.geminiModel : null;
    } catch {
      return null;
    }
  }

  function get() {
    const fromEnv = (env.NAVEGADOR_GEMINI_MODEL || '').trim();
    if (isValidModelName(fromEnv)) return fromEnv;
    return readSaved() ?? DEFAULT_GEMINI_MODEL;
  }

  return {
    get,
    /** Lo que la interfaz necesita: el modelo en uso y el de fabrica. */
    info() {
      return { model: get(), defaultModel: DEFAULT_GEMINI_MODEL };
    },
    set(name) {
      const trimmed = typeof name === 'string' ? name.trim() : name;
      if (!isValidModelName(trimmed)) {
        throw new Error('nombre de modelo no válido: usa solo minúsculas, cifras, punto y guion (por ejemplo gemini-3.5-flash)');
      }
      fs.writeFileSync(filePath, JSON.stringify({ geminiModel: trimmed }), 'utf8');
      return this.info();
    },
    reset() {
      try {
        fs.unlinkSync(filePath);
      } catch (err) {
        if (err.code !== 'ENOENT') throw err;
      }
      return this.info();
    },
  };
}

module.exports = { createModelSettings, isValidModelName, DEFAULT_GEMINI_MODEL };
