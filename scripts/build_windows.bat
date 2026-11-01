@echo off
setlocal enabledelayedexpansion

echo ======================================================
echo    Bifaria - Windows Build Script (.exe / .msi)
echo ======================================================
echo.

cd /d "%~dp0\.."

echo [1/3] Checking dependencies...
where bun >nul 2>nul
if %errorlevel% neq 0 (
    echo [!] Bun is not found. Trying npm...
    where npm >nul 2>nul
    if %errorlevel% neq 0 (
        echo [ERROR] Neither Bun nor Node/npm is installed. Please install Node.js or Bun.
        pause
        exit /b 1
    )
    set PKG_MGR=npm
) else (
    set PKG_MGR=bun
)

echo [2/3] Installing frontend packages using !PKG_MGR!...
if "!PKG_MGR!"=="bun" (
    call bun install
    call bun run build
    echo [3/3] Compiling Windows executable with Tauri...
    call bun run tauri build
) else (
    call npm install
    call npm run build
    echo [3/3] Compiling Windows executable with Tauri...
    call npx tauri build
)

echo.
echo ======================================================
echo    Build Completed!
echo ======================================================
echo Output installers:
echo  - Setup Installer (.exe): src-tauri\target\release\bundle\nsis\
echo  - Windows Installer (.msi): src-tauri\target\release\bundle\msi\
echo  - Standalone Binary (.exe): src-tauri\target\release\bifaria.exe
echo ======================================================
pause
