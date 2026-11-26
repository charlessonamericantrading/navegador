@echo off
title Navegador
cd /d "%~dp0"
if exist "%LOCALAPPDATA%\Programs\NavegadorIA\navegador.exe" (
    start "" "%LOCALAPPDATA%\Programs\NavegadorIA\navegador.exe" %*
    exit /b 0
)
if not exist "engine\target\release\navegador.exe" (
    echo Primero ejecuta instalar.bat
    pause
    exit /b 1
)
start "" "engine\target\release\navegador.exe" %*
