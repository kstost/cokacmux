@echo off
setlocal
set "ZIG=%~dp0tools\zig-0.13.0\zig.exe"
if not exist "%ZIG%" (
  echo zig.exe not found at "%ZIG%" 1>&2
  exit /b 1
)
set "ARGS="
:next
if "%~1"=="" goto run
set "ARG=%~1"
if /I "%ARG:~0,9%"=="--target=" (
  shift
  goto next
)
set ARGS=%ARGS% "%~1"
shift
goto next
:run
"%ZIG%" cc -target aarch64-windows-gnu %ARGS%
exit /b %ERRORLEVEL%
