const { contextBridge, ipcRenderer } = require('electron');

// Exponer un API seguro en window.electronAPI
contextBridge.exposeInMainWorld('electronAPI', {
  isElectron: true,
  platform: process.platform,

  // Si en el futuro necesitamos abrir enlaces externos en el navegador por defecto del PC
  openExternal: (url) => ipcRenderer.send('open-external', url),

  // Se dispara cuando el proceso del backend Python se reinicia o falla,
  // para que la interfaz pueda avisar al usuario en vez de quedarse colgada.
  onBackendStatus: (callback) => {
    const listener = (_event, data) => callback(data);
    ipcRenderer.on('backend-status', listener);
    return () => ipcRenderer.removeListener('backend-status', listener);
  },

  // Avisos de auto-actualización (disponible / ya descargada y lista para instalar)
  onUpdateStatus: (callback) => {
    const listener = (_event, data) => callback(data);
    ipcRenderer.on('update-status', listener);
    return () => ipcRenderer.removeListener('update-status', listener);
  },
  installUpdate: () => ipcRenderer.send('install-update')
});
