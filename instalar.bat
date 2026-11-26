@echo off
title Instalar el Navegador
setlocal
set "REL=%~dp0engine\target\release"
set "FORZAR="
if /i "%~1"=="--recompilar" (
    set "FORZAR=1"
    shift
)

if defined FORZAR goto compilar
if not exist "%REL%\navegador.exe" goto compilar
if not exist "%REL%\instalador.exe" goto compilar
goto instalar

:compilar
echo ===================================================
echo   Compilar e instalar el Navegador (100%% Rust)
echo ===================================================
echo.
cargo --version >nul 2>&1
if %errorlevel% neq 0 (
    echo [ERROR] Rust no esta instalado o cargo no esta en el PATH.
    echo Instala Rust desde https://rustup.rs/ y vuelve a ejecutar este archivo.
    start https://rustup.rs/
    pause
    exit /b 1
)
echo Compilando el navegador y el instalador (la primera vez tarda unos minutos)...
cargo build --release --locked --manifest-path "%~dp0engine\Cargo.toml" --bin navegador --bin instalador
if %errorlevel% neq 0 (
    echo [ERROR] No se pudo compilar el navegador.
    pause
    exit /b 1
)

:instalar
"%REL%\instalador.exe" --origen "%REL%\navegador.exe" %1 %2 %3 %4
if %errorlevel% neq 0 (
    echo [ERROR] Fallo al instalar el navegador.
    pause
    exit /b 1
)
exit /b 0
