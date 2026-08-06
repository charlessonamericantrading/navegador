@echo off
title Instalar Navegador para Agentes de IA
echo ===================================================
echo   Instalador del Navegador para Agentes de IA
echo ===================================================
echo.

:: Verificar si Node.js está instalado
node -v >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERROR] Node.js no esta instalado o no se encuentra en el PATH.
    echo Por favor, instala Node.js (se recomienda la version LTS) antes de continuar.
    echo Abriendo la pagina de descarga de Node.js...
    start https://nodejs.org/
    pause
    exit /b
)

:: Verificar si Python está instalado
python --version >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERROR] Python no esta instalado o no se encuentra en el PATH.
    echo Por favor, instala Python 3.10 o superior (y marca la opcion "Add Python to PATH").
    echo Abriendo la pagina de descarga de Python...
    start https://www.python.org/downloads/
    pause
    exit /b
)

echo [1/3] Instalando dependencias de Node en la raiz...
call npm install
if %errorlevel% neq 0 (
    echo [ERROR] Fallo la instalacion de dependencias raiz.
    pause
    exit /b
)

echo [2/3] Ejecutando instalacion completa (Frontend, Backend y Electron)...
echo Esto puede tomar unos minutos mientras se descargan las librerias...
call npm run install:all
if %errorlevel% neq 0 (
    echo [ERROR] Fallo la instalacion de las dependencias de la aplicacion.
    pause
    exit /b
)

echo [3/3] Creando acceso directo de ejecucion...
(
echo @echo off
echo title Iniciar Navegador para Agentes de IA
echo cd /d "%%~dp0"
echo echo Iniciando aplicacion...
echo call npm start
) > iniciar.bat

echo.
echo ===================================================
echo   ¡Instalacion completada con exito!
echo ===================================================
echo.
echo Ahora puedes hacer doble clic en el archivo "iniciar.bat"
echo en esta misma carpeta para arrancar la aplicacion.
echo.
pause
