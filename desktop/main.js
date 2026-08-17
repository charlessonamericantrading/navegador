const { app, BrowserWindow, shell, ipcMain, protocol, net } = require('electron');
const path = require('path');
const fs = require('fs');
const child_process = require('child_process');
const { autoUpdater } = require('electron-updater');

// Registrar el protocolo "app" como seguro y estándar para permitir ES Modules
protocol.registerSchemesAsPrivileged([
  { scheme: 'app', privileges: { secure: true, standard: true, supportFetchAPI: true } }
]);

// Evitar dos instancias simultáneas: el backend siempre escucha en el mismo puerto
// fijo (127.0.0.1:8000), así que una segunda instancia no arrancaría su propio
// backend aislado, sino que su ventana se conectaría en silencio al backend de la
// primera instancia, compartiendo la misma sesión de navegador sin avisar a nadie.
const gotSingleInstanceLock = app.requestSingleInstanceLock();
if (!gotSingleInstanceLock) {
  app.quit();
  return;
}

// Si alguien intenta abrir una segunda instancia, en vez de dejarla abrir su
// propia ventana (que acabaría hablando con el backend de la primera), traemos
// al frente la ventana ya existente.
app.on('second-instance', () => {
  if (mainWindow) {
    if (mainWindow.isMinimized()) mainWindow.restore();
    mainWindow.focus();
  }
});

let mainWindow = null;
let pythonProcess = null;
let nativeEngineProcess = null;
let nativeEngineStdoutBuffer = '';
let requestCounter = 0;
const pendingEngineRequests = new Map();
let isQuitting = false;
let restartAttempts = 0;
let stableTimer = null;
const MAX_RESTART_ATTEMPTS = 3;

function notifyBackendStatus(status, extra = {}) {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send('backend-status', { status, ...extra });
  }
}

function getNativeEnginePath() {
  const isWin = process.platform === 'win32';
  const isDev = !app.isPackaged;
  const nativeEngineName = isWin ? 'engine_server.exe' : 'engine_server';
  let enginePath = isDev
    ? path.join(__dirname, '..', 'engine', 'target', 'release', nativeEngineName)
    : path.join(process.resourcesPath, 'engine', nativeEngineName);

  if (isDev && !fs.existsSync(enginePath)) {
    const debugEnginePath = path.join(__dirname, '..', 'engine', 'target', 'debug', nativeEngineName);
    if (fs.existsSync(debugEnginePath)) {
      enginePath = debugEnginePath;
    }
  }
  return enginePath;
}

function sendEngineRequest(payload) {
  return new Promise((resolve, reject) => {
    if (!nativeEngineProcess || !nativeEngineProcess.stdin.writable) {
      return reject(new Error('El motor nativo Rust no está disponible'));
    }
    requestCounter += 1;
    const reqId = `ipc-${requestCounter}`;
    const message = { ...payload, id: reqId };

    const timeout = setTimeout(() => {
      pendingEngineRequests.delete(reqId);
      reject(new Error(`Timeout esperando respuesta del motor para '${payload.type}'`));
    }, 30000);

    pendingEngineRequests.set(reqId, { resolve, reject, timeout });
    try {
      nativeEngineProcess.stdin.write(JSON.stringify(message) + '\n');
    } catch (err) {
      clearTimeout(timeout);
      pendingEngineRequests.delete(reqId);
      reject(err);
    }
  });
}

function handleEngineLine(line) {
  if (!line.trim()) return;
  let parsed;
  try {
    parsed = JSON.parse(line);
  } catch (err) {
    console.error('[NativeEngine-parse-error]:', line.substring(0, 100), err);
    return;
  }

  // Notificar al frontend si es un estado o handshake de inicio
  if (parsed.type === 'state' || parsed.type === 'ready') {
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.webContents.send('engine:state', parsed);
    }
  }

  // Resolver la promesa de solicitud si coincide con el ID
  if (parsed.id && pendingEngineRequests.has(parsed.id)) {
    const { resolve, timeout } = pendingEngineRequests.get(parsed.id);
    clearTimeout(timeout);
    pendingEngineRequests.delete(parsed.id);
    resolve(parsed);
  }
}

function startNativeEngine() {
  const enginePath = getNativeEnginePath();
  if (!fs.existsSync(enginePath)) {
    console.warn(`[NativeEngine]: Binario Rust no encontrado en ${enginePath}`);
    notifyBackendStatus('failed', { message: 'Binario de motor Rust no encontrado' });
    return;
  }

  console.log(`[NativeEngine]: Iniciando motor Rust directamente desde ${enginePath}`);
  nativeEngineStdoutBuffer = '';

  nativeEngineProcess = child_process.spawn(enginePath, [], {
    windowsHide: true,
    stdio: ['pipe', 'pipe', 'pipe']
  });

  nativeEngineProcess.stdout.on('data', (chunk) => {
    nativeEngineStdoutBuffer += chunk.toString();
    const lines = nativeEngineStdoutBuffer.split('\n');
    nativeEngineStdoutBuffer = lines.pop(); // Mantener el fragmento incompleto al final
    for (const line of lines) {
      handleEngineLine(line);
    }
  });

  nativeEngineProcess.stderr.on('data', (chunk) => {
    console.error(`[NativeEngine-stderr]: ${chunk.toString().trim()}`);
  });

  nativeEngineProcess.on('error', (err) => {
    console.error('[NativeEngine]: Error en proceso:', err);
    notifyBackendStatus('failed', { message: err.message });
  });

  nativeEngineProcess.on('close', (code) => {
    console.log(`[NativeEngine]: Proceso cerrado con código ${code}`);
    nativeEngineProcess = null;
    for (const [id, req] of pendingEngineRequests.entries()) {
      clearTimeout(req.timeout);
      req.reject(new Error('El motor Rust se cerró'));
    }
    pendingEngineRequests.clear();
  });
}

function startPythonBackend() {
  const isWin = process.platform === 'win32';
  const isDev = !app.isPackaged;
  const nativeEnginePath = getNativeEnginePath();
  
  // Rutas al entorno virtual según el sistema operativo (Desarrollo)
  const venvPython = isWin
    ? path.join(__dirname, '..', 'backend', '.venv', 'Scripts', 'python.exe')
    : path.join(__dirname, '..', 'backend', '.venv', 'bin', 'python');
    
  const scriptPath = path.join(__dirname, '..', 'backend', 'app', 'core', 'main.py');
  const backendDir = path.join(__dirname, '..', 'backend');

  let exePath = '';
  let args = [];
  let cwd = '';

  if (!isDev) {
    // Modo producción (empaquetado)
    const exeName = isWin ? 'backend-server.exe' : 'backend-server';
    exePath = path.join(process.resourcesPath, 'backend-server', exeName);
    cwd = path.join(process.resourcesPath, 'backend-server');
    args = [];
    
    console.log(`Producción: Ejecutando servidor binario en ${exePath}`);
  } else {
    // Modo desarrollo
    exePath = venvPython;
    args = [scriptPath];
    cwd = backendDir;
    console.log(`Desarrollo: Ejecutando script con python en ${exePath}`);
  }

  const backendEnv = {
    ...process.env,
    PYTHONIOENCODING: 'utf-8',
    PYTHONUNBUFFERED: '1',
    NATIVE_ENGINE_PATH: nativeEnginePath
  };

  if (!fs.existsSync(exePath)) {
    console.warn(`[FastAPI]: Ejecutable Python no encontrado en ${exePath}. Usando exclusivamente motor Rust nativo directo.`);
    return;
  }

  pythonProcess = child_process.spawn(exePath, args, {
    cwd: cwd,
    windowsHide: true,
    detached: !isWin,
    env: backendEnv
  });

  pythonProcess.stdout.on('data', (data) => {
    console.log(`[FastAPI-stdout]: ${data.toString().trim()}`);
  });

  pythonProcess.stderr.on('data', (data) => {
    console.error(`[FastAPI-stderr]: ${data.toString().trim()}`);
  });

  clearTimeout(stableTimer);
  stableTimer = setTimeout(() => {
    restartAttempts = 0;
  }, 8000);

  let backendDownHandled = false;
  const handleBackendDown = () => {
    if (backendDownHandled) return;
    backendDownHandled = true;
    clearTimeout(stableTimer);

    if (isQuitting) return;

    if (restartAttempts < MAX_RESTART_ATTEMPTS) {
      restartAttempts += 1;
      notifyBackendStatus('restarting', { attempt: restartAttempts });
      const backoffMs = 1500 * restartAttempts;
      setTimeout(() => {
        if (!isQuitting) {
          startPythonBackend();
        }
      }, backoffMs);
    } else {
      notifyBackendStatus('failed');
    }
  };

  pythonProcess.on('error', (err) => {
    console.error(`No se pudo iniciar el proceso del backend (${exePath}):`, err);
    handleBackendDown();
  });

  pythonProcess.on('close', (code) => {
    console.log(`El proceso del backend en Python se cerró con código: ${code}`);
    handleBackendDown();
  });
}

// El backend (backend-server.exe / python) puede lanzar procesos hijos propios,
// que a su vez lancen los suyos. child.kill() solo mata ese PID directo: los
// nietos quedaban huérfanos y seguían corriendo en segundo plano tras cerrar
// la app - problema real encontrado con una dependencia previa de este
// proyecto que hacía justo eso, pero el mismo riesgo aplica a cualquier
// proceso que el backend lance en el futuro. En Windows no existen grupos de
// procesos POSIX, así que se usa "taskkill /t" para matar el árbol completo;
// en el resto de plataformas se aprovecha el grupo de procesos creado por
// "detached: true" en el spawn, matando el PID negativo (todo el grupo).
function killProcessTree(proc) {
  if (!proc || proc.pid == null) return;
  if (process.platform === 'win32') {
    try {
      child_process.spawnSync('taskkill', ['/pid', String(proc.pid), '/t', '/f']);
    } catch (err) {
      console.error('No se pudo terminar el árbol de procesos del backend:', err);
    }
  } else {
    try {
      process.kill(-proc.pid, 'SIGKILL');
    } catch (err) {
      try {
        proc.kill('SIGKILL');
      } catch (_) {
        // El proceso ya no existe; nada que hacer.
      }
    }
  }
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 1024,
    minHeight: 768,
    title: "AI Agent Browser",
    backgroundColor: '#090b11',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false
    }
  });

  // Quitar el menú estándar por defecto para dar un aspecto más nativo y moderno
  mainWindow.removeMenu();

  // En modo desarrollo, cargamos el servidor de Vite (con un pequeño retardo para dejar que levante)
  const isDev = !app.isPackaged;
  if (isDev) {
    setTimeout(() => {
      mainWindow.loadURL('http://localhost:5173').catch((err) => {
        console.error("No se pudo cargar el servidor de Vite, recargando...", err);
        mainWindow.loadURL('http://localhost:5173');
      });
    }, 3000); // 3 segundos de retardo en desarrollo para dar tiempo a Vite
  } else {
    // En producción, cargamos el archivo usando el protocolo seguro "app://"
    mainWindow.loadURL('app://./index.html').catch((err) => {
      console.error("No se pudo cargar el archivo HTML mediante el protocolo app://:", err);
    });
  }

  if (process.env.DEBUG_APP === 'true') {
    mainWindow.webContents.openDevTools();
  }

  mainWindow.on('closed', () => {
    mainWindow = null;
  });
}

function notifyUpdateStatus(status) {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send('update-status', { status });
  }
}

// La auto-actualización requiere que "build.publish" en package.json apunte a un
// repositorio real (ver desktop/DISTRIBUCION.md). Hasta entonces, checkForUpdates
// fallará silenciosamente: no debe interrumpir a personas que están usando la app.
function setupAutoUpdates() {
  autoUpdater.autoDownload = true;
  autoUpdater.autoInstallOnAppQuit = false;

  autoUpdater.on('update-available', () => notifyUpdateStatus('available'));
  autoUpdater.on('update-downloaded', () => notifyUpdateStatus('downloaded'));
  autoUpdater.on('error', (err) => {
    console.log(`Comprobación de actualizaciones no disponible (normal si aún no hay releases publicados): ${err}`);
  });

  autoUpdater.checkForUpdates().catch(() => {
    // Silencioso a propósito: sin "publish" configurado con un repositorio real,
    // esto fallará siempre y no debe mostrarse como un error de la app.
  });
}

ipcMain.on('install-update', () => {
  autoUpdater.quitAndInstall();
});

// Inicialización de Electron
app.whenReady().then(() => {
  // Manejador del protocolo para cargar los archivos del frontend en producción
  protocol.handle('app', (request) => {
    let relativePath = request.url.replace(/^app:\/\//, '');
    
    // Quitar slashes e indicador de ruta relativa iniciales
    if (relativePath.startsWith('/')) {
      relativePath = relativePath.slice(1);
    }
    if (relativePath.startsWith('./')) {
      relativePath = relativePath.slice(2);
    }

    // Cargar index.html si está vacío
    if (relativePath === '' || relativePath === '/') {
      relativePath = 'index.html';
    }

    // Resolver ruta absoluta en recursos empaquetados
    const baseDir = path.join(process.resourcesPath, 'frontend', 'dist');
    const absolutePath = path.normalize(path.join(baseDir, relativePath));

    if (!absolutePath.startsWith(baseDir)) {
      return new Response('Forbidden', { status: 403 });
    }

    const formattedPath = absolutePath.replace(/\\/g, '/');
    return net.fetch('file:///' + formattedPath);
  });

  // Arrancar el motor nativo Rust directamente (prioridad)
  startNativeEngine();

  // Arrancar opcionalmente el backend de Python para retrocompatibilidad
  startPythonBackend();

  // Crear la ventana principal de la app
  createWindow();

  // La auto-actualización solo tiene sentido en la app empaquetada e instalada
  if (app.isPackaged) {
    setupAutoUpdates();
  }

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });
});

// Registrar eventos IPC expuestos
ipcMain.handle('engine:request', async (_event, payload) => {
  return await sendEngineRequest(payload);
});

ipcMain.on('open-external', (event, url) => {
  if (typeof url === 'string' && /^(https?|mailto):/i.test(url)) {
    shell.openExternal(url);
  } else {
    console.warn('open-external bloqueado: esquema no permitido ->', url);
  }
});

// Asegurarse de cerrar procesos al salir de la aplicación de escritorio
app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit();
  }
});

app.on('will-quit', () => {
  isQuitting = true;
  if (nativeEngineProcess) {
    console.log("Cerrando el motor Rust nativo...");
    try {
      nativeEngineProcess.stdin.write('{"type":"shutdown","id":"quit"}\n');
    } catch (_) {}
    killProcessTree(nativeEngineProcess);
    nativeEngineProcess = null;
  }
  if (pythonProcess) {
    console.log("Cerrando el árbol de procesos del backend...");
    killProcessTree(pythonProcess);
    pythonProcess = null;
  }
});
