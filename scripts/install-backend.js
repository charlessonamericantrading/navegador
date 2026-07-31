const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');

const backendDir = path.join(__dirname, '..', 'backend');
const isWin = process.platform === 'win32';

function runCmd(cmd, cwd) {
  console.log(`Ejecutando: ${cmd}`);
  execSync(cmd, { cwd, stdio: 'inherit' });
}

try {
  console.log('=== Configurando entorno de Python para el Backend ===');

  // 1. Crear entorno virtual si no existe
  const venvDir = path.join(backendDir, '.venv');
  if (!fs.existsSync(venvDir)) {
    console.log('Creando entorno virtual (.venv)...');
    runCmd('python -m venv .venv', backendDir);
  } else {
    console.log('El entorno virtual (.venv) ya existe.');
  }

  // 2. Determinar rutas de ejecutables según plataforma
  const pipName = isWin ? 'pip.exe' : 'pip';
  const playwrightName = isWin ? 'playwright.exe' : 'playwright';
  
  const pipPath = path.join(venvDir, isWin ? 'Scripts' : 'bin', pipName);
  const playwrightPath = path.join(venvDir, isWin ? 'Scripts' : 'bin', playwrightName);

  // 3. Instalar requerimientos pip
  console.log('Instalando dependencias de Python (requirements.txt)...');
  runCmd(`"${pipPath}" install -r requirements.txt`, backendDir);

  // 4. Instalar navegador Chromium para Playwright
  console.log('Descargando binarias de Chromium para Playwright...');
  runCmd(`"${playwrightPath}" install chromium`, backendDir);

  console.log('=== Entorno de Python configurado con éxito ===');
} catch (error) {
  console.error('Error durante la instalación del backend:', error.message);
  process.exit(1);
}
