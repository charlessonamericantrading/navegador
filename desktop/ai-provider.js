// Llamada al proveedor de IA desde el proceso principal (plan F05, H04). El
// renderer manda el prompt ya construido y recibe el texto del modelo; la
// clave no sale de aquí.
//
// Cada petición lleva un `requestId` del renderer para poder cancelarla: el
// botón «Detener» del agente aborta la petición HTTP, no solo deja de esperarla.

const GEMINI_URL = 'https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent';
const MAX_PROMPT_CHARS = 200_000;

/** Quita la clave de cualquier texto que vaya a salir del proceso principal. */
function redact(text, key) {
  const value = String(text);
  return key ? value.split(key).join('[clave oculta]') : value;
}

function createGeminiProvider({ getKey, fetch }) {
  const inFlight = new Map();

  async function generate(requestId, prompt) {
    if (typeof requestId !== 'string' || !requestId || requestId.length > 100) throw new TypeError('requestId no válido');
    if (typeof prompt !== 'string' || !prompt) throw new TypeError('prompt no válido');
    if (prompt.length > MAX_PROMPT_CHARS) throw new Error(`prompt demasiado largo (${prompt.length} caracteres)`);
    if (inFlight.has(requestId)) throw new Error('requestId repetido');

    const key = getKey();
    if (!key) throw new Error('No hay clave de Gemini configurada: introdúcela en Ajustes.');

    const controller = new AbortController();
    inFlight.set(requestId, controller);
    try {
      // La clave va en cabecera, no en la URL: una URL acaba en logs y trazas.
      const response = await fetch(GEMINI_URL, {
        method: 'POST',
        signal: controller.signal,
        headers: { 'Content-Type': 'application/json', 'x-goog-api-key': key },
        body: JSON.stringify({
          contents: [{ role: 'user', parts: [{ text: prompt }] }],
          generationConfig: { responseMimeType: 'application/json', temperature: 0.2 },
        }),
      });
      if (!response.ok) {
        const detail = (await response.text()).slice(0, 500);
        throw new Error(`API error (${response.status}): ${detail}`);
      }
      const data = await response.json();
      return data?.candidates?.[0]?.content?.parts?.[0]?.text ?? '';
    } catch (err) {
      if (controller.signal.aborted) throw new Error('cancelado');
      throw new Error(redact(err instanceof Error ? err.message : err, key));
    } finally {
      inFlight.delete(requestId);
    }
  }

  function cancel(requestId) {
    inFlight.get(requestId)?.abort();
  }

  return { generate, cancel };
}

module.exports = { createGeminiProvider, redact, GEMINI_URL };
