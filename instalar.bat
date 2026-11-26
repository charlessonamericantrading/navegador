@echo off
title Instalar el Navegador
echo ===================================================
echo   Instalar el Navegador (100%% Rust)
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

echo Compilando el navegador y el instalador Rust (la primera vez tarda unos minutos)...
cargo build --release --locked --manifest-path "%~dp0engine\Cargo.toml" --bin navegador --bin instalador
if %errorlevel% neq 0 (
    echo [ERROR] No se pudo compilar el navegador.
    pause
    exit /b 1
)

"%~dp0engine\target\release\instalador.exe" --origen "%~dp0engine\target\release\navegador.exe" --silencioso
if %errorlevel% neq 0 (
    echo [ERROR] Fallo al registrar la instalacion.
    pause
    exit /b 1
)

echo.
echo Listo. Abre el navegador desde el Menu Inicio o con iniciar.bat
pause
