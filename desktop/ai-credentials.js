// Almacén de la clave del proveedor de IA en el proceso principal (plan F05,
// hallazgo H04). Antes la clave vivía en `localStorage` del renderer y el
// renderer llamaba al proveedor con ella en la URL.
//
// Reglas:
// - La clave entra por `set` y nunca vuelve a salir hacia el renderer: `status`
//   solo dice si hay una y si se guardó cifrada.
// - En disco solo se escribe cifrada con `safeStorage`. Si el sistema no ofrece
//   un almacén seguro de verdad (en Linux, el backend `basic_text` "cifra" con
//   una clave fija), la clave se queda en memoria durante la sesión y no se
//   persiste. Pedirla otra vez es mejor que guardarla en claro.
//
// Es una fábrica con sus dependencias inyectadas para poder probarla con
// `node --test` sin arrancar Electron.

function createCredentialStore({ safeStorage, filePath, fs, platform }) {
  let sessionOnlyKey = null;

  function secureStorageAvailable() {
    if (!safeStorage.isEncryptionAvailable()) return false;
    if (platform === 'linux' && typeof safeStorage.getSelectedStorageBackend === 'function') {
      const backend = safeStorage.getSelectedStorageBackend();
      return backend !== 'basic_text' && backend !== 'unknown';
    }
    return true;
  }

  function readPersisted() {
    try {
      const data = JSON.parse(fs.readFileSync(filePath, 'utf8'));
      if (typeof data.gemini !== 'string') return null;
      return safeStorage.decryptString(Buffer.from(data.gemini, 'base64'));
    } catch {
      // Sin fichero, fichero ajeno o cifrado de otra máquina/usuario: no hay
      // clave utilizable, que es distinto de un error que mostrar.
      return null;
    }
  }

  function removePersisted() {
    try {
      fs.unlinkSync(filePath);
    } catch (err) {
      if (err.code !== 'ENOENT') throw err;
    }
  }

  return {
    /** Lo único que el renderer puede saber de la clave. */
    status() {
      const persisted = readPersisted() !== null;
      return { configured: persisted || sessionOnlyKey !== null, persisted, secureStorage: secureStorageAvailable() };
    },

    set(key) {
      if (typeof key !== 'string') throw new TypeError('la clave debe ser texto');
      const trimmed = key.trim();
      if (!trimmed) throw new Error('la clave está vacía');
      if (trimmed.length > 512) throw new Error('la clave es demasiado larga');

      if (secureStorageAvailable()) {
        const cipher = safeStorage.encryptString(trimmed).toString('base64');
        fs.writeFileSync(filePath, JSON.stringify({ gemini: cipher }), { mode: 0o600 });
        sessionOnlyKey = null;
      } else {
        removePersisted();
        sessionOnlyKey = trimmed;
      }
      return this.status();
    },

    clear() {
      sessionOnlyKey = null;
      removePersisted();
      return this.status();
    },

    /** Solo para el proceso principal: nunca se expone por IPC. */
    get() {
      return sessionOnlyKey ?? readPersisted();
    },
  };
}

module.exports = { createCredentialStore };
