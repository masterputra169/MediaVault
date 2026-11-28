@echo off
rem ---------------------------------------------------------------------------
rem MediaVault build helper (Windows).
rem
rem MediaVault links BoringSSL (via wreq/btls) to impersonate real browser TLS,
rem which needs the MSVC toolchain plus CMake, Ninja, NASM and libclang.
rem CMake/Ninja ship inside Visual Studio; NASM and LLVM are looked up from
rem NASM_DIR / LIBCLANG_PATH when those are set.
rem
rem Usage: build-env.bat check | build --release | test
rem ---------------------------------------------------------------------------
setlocal EnableExtensions

if "%~1"=="" (
    echo usage: build-env.bat ^<cargo args^>
    exit /b 2
)

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "VSROOT="
if exist "%VSWHERE%" (
    for /f "usebackq delims=" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSROOT=%%i"
)
if not defined VSROOT (
    echo [build-env] Visual Studio with the C++ workload was not found.
    echo [build-env] Install "Desktop development with C++" from the Visual Studio installer.
    exit /b 1
)

call "%VSROOT%\Common7\Tools\VsDevCmd.bat" -arch=amd64 -host_arch=amd64 >nul 2>&1
if errorlevel 1 (
    echo [build-env] VsDevCmd.bat failed.
    exit /b 1
)

rem CMake and Ninja are bundled with Visual Studio.
set "PATH=%VSROOT%\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin;%VSROOT%\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;%PATH%"

rem Git-for-Windows / Laragon ship a POSIX link.exe that shadows the MSVC linker.
set "PATH=%PATH:C:\Program Files\Git\usr\bin;=%"
set "PATH=%PATH:C:\laragon\bin\git\usr\bin;=%"

if defined NASM_DIR set "PATH=%NASM_DIR%;%PATH%"
if defined LIBCLANG_PATH set "PATH=%LIBCLANG_PATH%;%PATH%"

where nasm >nul 2>&1
if errorlevel 1 echo [build-env] WARNING: nasm not found. Set NASM_DIR before building, BoringSSL needs it.
if not defined LIBCLANG_PATH echo [build-env] WARNING: LIBCLANG_PATH is not set. Set it to a directory containing libclang.dll.

cargo %*
exit /b %ERRORLEVEL%
