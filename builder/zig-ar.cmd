@echo off
setlocal
set "ZIG=%~dp0tools\zig-0.13.0\zig.exe"
if not exist "%ZIG%" (
  echo zig.exe not found at "%ZIG%" 1>&2
  exit /b 1
)
"%ZIG%" ar %*
exit /b %ERRORLEVEL%
