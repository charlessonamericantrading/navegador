import type { ModelProvider } from './AgentOrchestrator';

/**
 * Proveedor de modelo respaldado por el proceso principal de Electron (plan
 * H04). Cada petición lleva un `requestId` propio para que «Detener» pueda
 * abortar la petición HTTP allí, no solo dejar de esperarla aquí.
 *
 * `undefined` fuera de Electron: sin proceso principal no hay dónde guardar
 * la clave de forma segura, y el orquestador lo reporta como fallo.
 */
export function electronModelProvider(): ModelProvider | undefined {
  const ai = window.electronAPI?.ai;
  if (!ai) return undefined;

  return {
    async generate(prompt, signal) {
      const requestId = crypto.randomUUID();
      const onAbort = () => {
        ai.cancel(requestId).catch(() => {
          // La petición pudo terminar entre medias: no hay nada que cancelar.
        });
      };
      signal?.addEventListener('abort', onAbort, { once: true });
      try {
        return await ai.generate(requestId, prompt);
      } finally {
        signal?.removeEventListener('abort', onAbort);
      }
    },
  };
}
