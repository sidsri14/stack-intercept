@echo off
setlocal

set MSVC_ROOT=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207
set WINSDK_ROOT=C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0

set LIB=%MSVC_ROOT%\lib\x64;%WINSDK_ROOT%\um\x64;%WINSDK_ROOT%\ucrt\x64
set PATH=%MSVC_ROOT%\bin\Hostx64\x64;%PATH%

cargo %*
if %ERRORLEVEL% NEQ 0 exit /b %ERRORLEVEL%
