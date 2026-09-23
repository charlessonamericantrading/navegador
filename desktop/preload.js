const { contextBridge, ipcRenderer } = require('electron');

// Exponer un API seguro en window.electronAPI
contextBridge.exposeInMainWorld('electronAPI', {
  isElectron: true,
  platform: process.platform,

  // Comunicación IPC directa de ultra-baja latencia con el motor nativo Rust
  sendEngineRequest: (payload) => ipcRenderer.invoke('engine:request', payload),
  onEngineState: (callback) => {
    const listener = (_event, data) => callback(data);
    ipcRenderer.on('engine:state', listener);
    return () => ipcRenderer.removeListener('engine:state', listener);
  },

  // Si en el futuro necesitamos abrir enlaces externos en el navegador por defecto del PC
  openExternal: (url) => ipcRenderer.send('open-external', url),

  // Se dispara cuando el proceso del backend se reinicia o falla
  onBackendStatus: (callback) => {
    const listener = (_event, data) => callback(data);
    ipcRenderer.on('backend-status', listener);
    return () => ipcRenderer.removeListener('backend-status', listener);
  },

  // Avisos de auto-actualización
  onUpdateStatus: (callback) => {
    const listener = (_event, data) => callback(data);
    ipcRenderer.on('update-status', listener);
    return () => ipcRenderer.removeListener('update-status', listener);
  },
  installUpdate: () => ipcRenderer.send('install-update'),

  // IA (plan H04): la clave se entrega al proceso principal y no vuelve; el
  // renderer solo sabe si hay una. Las peticiones al modelo las hace el
  // proceso principal.
  ai: {
    credentialStatus: () => ipcRenderer.invoke('ai:credentials:status'),
    setGeminiKey: (key) => ipcRenderer.invoke('ai:credentials:set', key),
    clearGeminiKey: () => ipcRenderer.invoke('ai:credentials:clear'),
    generate: (requestId, prompt) => ipcRenderer.invoke('ai:generate', requestId, prompt),
    cancel: (requestId) => ipcRenderer.invoke('ai:cancel', requestId),
  },
});
