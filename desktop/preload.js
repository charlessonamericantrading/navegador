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
  installUpdate: () => ipcRenderer.send('install-update')
});
