const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const os = require('os');

const rootDir = path.join(__dirname, '..');
const frontendDir = path.join(rootDir, 'frontend');
const backendDir = path.join(rootDir, 'backend');
const desktopDir = path.join(rootDir, 'desktop');
const buildResourcesDir = path.join(desktopDir, 'build-resources');

const isWin = process.platform === 'win32';

function runCmd(cmd, cwd) {
  console.log(`\n>>> Ejecutando: ${cmd} (en ${cwd})`);
  execSync(cmd, { cwd, stdio: 'inherit' });
}

// Determina qué revisiones de Chromium usa realmente el Playwright instalado
// en el venv del backend (backend/.venv/.../playwright/driver/package/browsers.json),
// para poder excluir del paquete cualquier revisión vieja que haya quedado
// acumulada en la caché local de Playwright (p. ej. tras una actualización de
// versión). Sin este filtro, "npm run build:app" copia la carpeta ms-playwright
// completa tal cual esté en la máquina de quien compila, arrastrando cientos de
// MB de navegadores obsoletos a cada instalador generado.
function getPinnedChromiumRevisions(backendDir) {
  let browsersJsonPath = '';
  if (isWin) {
    browsersJsonPath = path.join(backendDir, '.venv', 'Lib', 'site-packages', 'playwright', 'driver', 'package', 'browsers.json');
  } else {
    const venvLibDir = path.join(backendDir, '.venv', 'lib');
    const pyDir = fs.existsSync(venvLibDir) ? fs.readdirSync(venvLibDir).find(d => d.startsWith('python')) : null;
    if (pyDir) {
      browsersJsonPath = path.join(venvLibDir, pyDir, 'site-packages', 'playwright', 'driver', 'package', 'browsers.json');
    }
  }

  if (!browsersJsonPath || !fs.existsSync(browsersJsonPath)) {
    console.log('[ADVERTENCIA] No se encontró browsers.json de Playwright; se copiará ms-playwright sin filtrar revisiones.');
    return null;
  }

  const browsersData = JSON.parse(fs.readFileSync(browsersJsonPath, 'utf-8'));
  return new Set(
    browsersData.browsers
      .filter(b => b.name === 'chromium' || b.name === 'chromium-headless-shell')
      .map(b => b.revision)
  );
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

try {
  console.log('===================================================');
  console.log('  PROCESO DE COMPILACIÓN Y EMPAQUETADO COMPLETO    ');
  console.log('===================================================');

  // Preparar caché para evitar problemas de symlinks en Windows
  prepareWinCodeSignCache();

  // 1. Compilar Frontend
  console.log('\n[Paso 1/5] Compilando Frontend (Vite + React)...');
  runCmd('npm run build', frontendDir);

  // 2. Compilar Backend a binario con PyInstaller
  console.log('\n[Paso 2/5] Compilando Backend (PyInstaller)...');
  const pyinstallerPath = isWin
    ? path.join(backendDir, '.venv', 'Scripts', 'pyinstaller.exe')
    : path.join(backendDir, '.venv', 'bin', 'pyinstaller');

  if (!fs.existsSync(pyinstallerPath)) {
    throw new Error(`No se encontró PyInstaller en: ${pyinstallerPath}. Por favor ejecuta la instalación primero.`);
  }

  // Ejecutamos pyinstaller con recopilación de todas las librerías dinámicas necesarias
  const pyinstallerCmd = `"${pyinstallerPath}" --onedir --noconfirm --clean --name backend-server --distpath dist --workpath build --collect-all uvicorn --collect-all fastapi --collect-all playwright --collect-all websockets --collect-all google --collect-all pydantic app/core/main.py`;
  runCmd(pyinstallerCmd, backendDir);

  // 3. Limpiar y recrear directorio de recursos temporales de Electron
  console.log('\n[Paso 3/5] Preparando carpeta de recursos de compilación...');
  if (fs.existsSync(buildResourcesDir)) {
    console.log('Limpiando recursos antiguos...');
    fs.rmSync(buildResourcesDir, { recursive: true, force: true });
  }
  fs.mkdirSync(buildResourcesDir, { recursive: true });

  // Copiar el backend compilado
  const compiledBackendSrc = path.join(backendDir, 'dist', 'backend-server');
  const compiledBackendDest = path.join(buildResourcesDir, 'backend-server');
  console.log(`Copiando servidor compilado desde ${compiledBackendSrc} a ${compiledBackendDest}...`);
  fs.cpSync(compiledBackendSrc, compiledBackendDest, { recursive: true });

  // 4. Copiar carpeta de Chromium de Playwright
  console.log('\n[Paso 4/5] Copiando carpeta local de navegadores Playwright...');
  let playwrightBrowsersPath = '';
  if (isWin) {
    playwrightBrowsersPath = path.join(process.env.LOCALAPPDATA, 'ms-playwright');
  } else if (process.platform === 'darwin') {
    playwrightBrowsersPath = path.join(os.homedir(), 'Library', 'Caches', 'ms-playwright');
  } else {
    playwrightBrowsersPath = path.join(os.homedir(), '.cache', 'ms-playwright');
  }

  const playwrightDest = path.join(buildResourcesDir, 'ms-playwright');
  if (fs.existsSync(playwrightBrowsersPath)) {
    console.log(`Copiando navegadores desde ${playwrightBrowsersPath} a ${playwrightDest}...`);
    const pinnedRevisions = getPinnedChromiumRevisions(backendDir);
    fs.cpSync(playwrightBrowsersPath, playwrightDest, {
      recursive: true,
      filter: (src) => {
        const base = path.basename(src);
        if (base === '.links') return false; // metadata local de la máquina, inútil en el paquete

        // backend/app/domains/browser/browser.py siempre lanza
        // chromium.launch(headless=True) sin "channel". Verificado empíricamente
        // (2026-07-31): con esa combinación, Playwright 1.60 ejecuta
        // chrome-headless-shell.exe (carpeta chromium_headless_shell-<rev>) y NUNCA
        // el chrome.exe de la carpeta chromium-<rev> completa. Esa carpeta pesa
        // ~413MB y viaja en el instalador sin que la app la use jamás, así que se
        // excluye del todo. Si algún día se lanza con headless=False o con
        // channel="chromium", quitar esta exclusión.
        if (/^chromium-\d+$/.test(base)) return false;

        const match = base.match(/^chromium(?:_headless_shell)?-(\d+)$/);
        if (!match) return true; // no es una carpeta de revisión de chromium: copiar tal cual
        if (!pinnedRevisions) return true; // no se pudo determinar qué se necesita: no filtrar por seguridad
        return pinnedRevisions.has(match[1]);
      }
    });
  } else {
    console.log(`[ADVERTENCIA] No se encontró la carpeta de navegadores Playwright en ${playwrightBrowsersPath}.`);
    console.log('Se intentará compilar de todas formas (puede que falten navegadores offline).');
  }

  // 5. Empaquetar con electron-builder
  console.log('\n[Paso 5/5] Empaquetando instalador con electron-builder...');
  runCmd('npx electron-builder', desktopDir);

  // Copiar el instalador generado a la raíz del proyecto para mayor accesibilidad
  const installerName = isWin ? 'Navegador IA Setup 1.0.0.exe' : (process.platform === 'darwin' ? 'Navegador IA-1.0.0.dmg' : 'navegador-ia-desktop_1.0.0_amd64.deb');
  const compiledInstallerPath = path.join(desktopDir, 'dist', installerName);
  const destInstallerPath = path.join(rootDir, isWin ? 'Navegador IA Setup.exe' : installerName);

  if (fs.existsSync(compiledInstallerPath)) {
    console.log(`\nCopiando el instalador generado a la raíz del proyecto...`);
    fs.copyFileSync(compiledInstallerPath, destInstallerPath);
    console.log(`¡Instalador disponible en la raíz!: ${destInstallerPath}`);
  }

  console.log('\n===================================================');
  console.log('  ¡EMPAQUETADO FINALIZADO CON ÉXITO!               ');
  console.log('  El instalador gráfico está listo en la raíz de la carpeta del proyecto. ');
  console.log('===================================================');
} catch (error) {
  console.error('\n[ERROR CRÍTICO] Falló el proceso de compilación:', error.message);
  process.exit(1);
}
