// Compila y empaqueta la aplicación de escritorio.
//
//   npm run build:app                  -> interfaz + motor Rust + Electron
//   npm run build:app -- --with-python-backend
//                                      -> además, el backend FastAPI opcional
//   npm run build:app -- --publish always
//                                      -> además, sube el release (ver
//                                         desktop/DISTRIBUCION.md)
//
// El backend Python ya no es obligatorio (plan F01, hallazgo H18): Electron
// solo lo arranca con USE_PYTHON_BACKEND=true, así que exigir PyInstaller y un
// `.venv` para construir el navegador normal no tenía sentido. Cuando se pide,
// el backend se añade a `extraResources` desde aquí, con la API de
// `electron-builder`, en vez de figurar siempre en `desktop/package.json`.
const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');

const rootDir = path.join(__dirname, '..');
const frontendDir = path.join(rootDir, 'frontend');
const backendDir = path.join(rootDir, 'backend');
const desktopDir = path.join(rootDir, 'desktop');
const buildResourcesDir = path.join(desktopDir, 'build-resources');

const isWin = process.platform === 'win32';
const withPythonBackend = process.argv.includes('--with-python-backend');
// Publicar es un paso aparte y explicito (`--publish always`, ver
// desktop/DISTRIBUCION.md): por defecto se construye sin subir nada.
const publishIndex = process.argv.indexOf('--publish');
const publish = publishIndex === -1 ? 'never' : process.argv[publishIndex + 1];
if (!['never', 'always', 'onTag', 'onTagOrDraft'].includes(publish)) {
  console.error(`--publish no valido: ${publish}`);
  process.exit(2);
}

function runCmd(cmd, cwd) {
  console.log(`\n>>> Ejecutando: ${cmd} (en ${cwd})`);
  execSync(cmd, { cwd, stdio: 'inherit' });
}

function prepareWinCodeSignCache() {
  if (process.platform !== 'win32') return;

  const cacheDir = path.join(process.env.LOCALAPPDATA, 'electron-builder', 'Cache', 'winCodeSign');
  if (!fs.existsSync(cacheDir)) {
    fs.mkdirSync(cacheDir, { recursive: true });
  }

  const targetDir = path.join(cacheDir, 'winCodeSign-2.6.0');
  if (fs.existsSync(targetDir)) {
    console.log('La cache de winCodeSign-2.6.0 ya existe.');
    return;
  }

  console.log('\n[Preparación] Configurando cache de winCodeSign para evitar errores de symlink en Windows...');

  // Buscar si ya hay una carpeta numérica con los archivos extraídos
  const items = fs.readdirSync(cacheDir);
  let sourceDir = '';

  for (const item of items) {
    const itemPath = path.join(cacheDir, item);
    if (fs.statSync(itemPath).isDirectory() && item !== 'winCodeSign-2.6.0') {
      if (fs.existsSync(path.join(itemPath, 'rcedit-x64.exe'))) {
        sourceDir = itemPath;
        break;
      }
    }
  }

  if (sourceDir) {
    console.log(`Encontrada carpeta extraída previamente en: ${sourceDir}`);
    console.log(`Copiando a: ${targetDir}`);
    fs.cpSync(sourceDir, targetDir, { recursive: true });
    console.log('¡Cache de winCodeSign configurada con éxito!');
    return;
  }

  // Si no hay carpeta numérica válida, buscar un .7z y extraerlo
  const z7Files = items.filter(f => f.endsWith('.7z'));
  if (z7Files.length > 0) {
    const archivePath = path.join(cacheDir, z7Files[0]);
    const exe7za = path.join(rootDir, 'desktop', 'node_modules', '7zip-bin', 'win', 'x64', '7za.exe');

    if (fs.existsSync(exe7za)) {
      console.log(`Intentando extraer ${archivePath} con 7za...`);
      try {
        execSync(`"${exe7za}" x -bd "${archivePath}" "-o${targetDir}"`, { stdio: 'ignore' });
      } catch (err) {
        // Ignoramos el error intencionadamente (el error ocurre por los symlinks de darwin)
      }
      if (fs.existsSync(path.join(targetDir, 'rcedit-x64.exe'))) {
        console.log('¡Cache de winCodeSign configurada con éxito tras extracción!');
        return;
      }
    }
  }

  console.log('[ADVERTENCIA] No se pudo pre-configurar la cache de winCodeSign. El build podría fallar si no se ejecuta como Administrador.');
}

function buildPythonBackend() {
  const pythonPath = isWin
    ? path.join(backendDir, '.venv', 'Scripts', 'python.exe')
    : path.join(backendDir, '.venv', 'bin', 'python');

  if (!fs.existsSync(pythonPath)) {
    throw new Error(`--with-python-backend necesita el entorno virtual en ${pythonPath}. Ejecuta antes \`npm run install:backend\`.`);
  }

  // Invocamos PyInstaller como módulo de Python (`python -m PyInstaller`) en vez del
  // ejecutable "pyinstaller.exe" generado por pip: ese stub quedó roto (sale con código 1
  // sin imprimir nada), mientras que el módulo importa y funciona con normalidad.
  const pyinstallerCmd = `"${pythonPath}" -m PyInstaller --onedir --noconfirm --clean --name backend-server --distpath dist --workpath build --paths . --collect-all uvicorn --collect-all fastapi --collect-all websockets --collect-all google --collect-all pydantic app/core/main.py`;
  runCmd(pyinstallerCmd, backendDir);

  const src = path.join(backendDir, 'dist', 'backend-server');
  const dest = path.join(buildResourcesDir, 'backend-server');
  console.log(`Copiando servidor compilado desde ${src} a ${dest}...`);
  fs.cpSync(src, dest, { recursive: true });
}

/** Instaladores del build actual, por la versión del manifiesto. */
function findInstallers(version) {
  const distDir = path.join(desktopDir, 'dist');
  if (!fs.existsSync(distDir)) return [];
  return fs.readdirSync(distDir)
    .filter((f) => f.includes(version) && /\.(exe|dmg|deb|AppImage|rpm)$/.test(f))
    .map((f) => path.join(distDir, f));
}

async function main() {
  const desktopPkg = JSON.parse(fs.readFileSync(path.join(desktopDir, 'package.json'), 'utf8'));
  const steps = withPythonBackend ? 5 : 4;

  console.log('===================================================');
  console.log('  PROCESO DE COMPILACIÓN Y EMPAQUETADO COMPLETO    ');
  console.log(`  Versión ${desktopPkg.version}${withPythonBackend ? ' (con backend Python)' : ''}`);
  console.log('===================================================');

  prepareWinCodeSignCache();

  console.log(`\n[Paso 1/${steps}] Compilando Frontend (Vite + React)...`);
  runCmd('npm run build', frontendDir);

  // `--locked`: el binario que se distribuye se compila con las versiones
  // exactas de `Cargo.lock`, las mismas que auditó `cargo audit`.
  console.log(`\n[Paso 2/${steps}] Compilando motor nativo Rust...`);
  runCmd('cargo build --manifest-path engine/Cargo.toml -p engine-core --bin engine_server --bin engine_broker --release --locked', rootDir);

  console.log(`\n[Paso 3/${steps}] Preparando carpeta de recursos de compilación...`);
  if (fs.existsSync(buildResourcesDir)) {
    fs.rmSync(buildResourcesDir, { recursive: true, force: true });
  }
  fs.mkdirSync(buildResourcesDir, { recursive: true });

  // El motor y el broker (Fase 67) viajan juntos: el motor no arranca sin él.
  const nativeEngineDestDir = path.join(buildResourcesDir, 'engine');
  fs.mkdirSync(nativeEngineDestDir, { recursive: true });
  for (const binary of ['engine_server', 'engine_broker']) {
    const nativeEngineName = isWin ? `${binary}.exe` : binary;
    const nativeEngineSrc = path.join(rootDir, 'engine', 'target', 'release', nativeEngineName);
    if (!fs.existsSync(nativeEngineSrc)) {
      throw new Error(`No se encontró el binario Rust en: ${nativeEngineSrc}`);
    }
    fs.copyFileSync(nativeEngineSrc, path.join(nativeEngineDestDir, nativeEngineName));
  }

  // Solo el DELTA respecto a `desktop/package.json`: electron-builder lee
  // igualmente el campo `build` y le fusiona este objeto, y las listas las
  // CONCATENA. Pasar la configuracion entera duplicaba `files` y
  // `extraResources`, y las dos copias simultaneas del motor chocaban (EBUSY).
  const configDelta = {};
  if (withPythonBackend) {
    console.log(`\n[Paso 4/${steps}] Compilando Backend opcional (PyInstaller)...`);
    buildPythonBackend();
    configDelta.extraResources = [{ from: 'build-resources/backend-server', to: 'backend-server', filter: ['**/*'] }];
  }

  console.log(`\n[Paso ${steps}/${steps}] Empaquetando instalador con electron-builder...`);
  const builder = require(path.join(desktopDir, 'node_modules', 'electron-builder'));
  await builder.build({ projectDir: desktopDir, config: configDelta, publish });

  // Copia de cortesía en la raíz, con el nombre real que generó el build (antes
  // se buscaba un nombre con `1.0.0` fijo, que dejaba de coincidir al subir la
  // versión).
  const installers = findInstallers(desktopPkg.version);
  for (const installer of installers) {
    const dest = path.join(rootDir, path.basename(installer));
    fs.copyFileSync(installer, dest);
    console.log(`Instalador disponible en: ${dest}`);
  }
  if (installers.length === 0) {
    console.log('[ADVERTENCIA] No se encontró ningún instalador con la versión del manifiesto en desktop/dist.');
  }

  console.log('\n===================================================');
  console.log('  ¡EMPAQUETADO FINALIZADO CON ÉXITO!               ');
  console.log('===================================================');
}

main().catch((error) => {
  console.error('\n[ERROR CRÍTICO] Falló el proceso de compilación:', error.message);
  process.exit(1);
});
