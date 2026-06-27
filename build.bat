@echo off
setlocal

REM Set up MSVC environment
set "MSVC_ROOT=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207"
set "WINSDK_ROOT=C:\Program Files (x86)\Windows Kits\10\Lib\10.0.26100.0"
set "WINSDK_INCLUDE=C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0"

REM Set LIB for the linker to find import libraries
set "LIB=%MSVC_ROOT%\lib\x64;%WINSDK_ROOT%\um\x64;%WINSDK_ROOT%\ucrt\x64"

REM Set INCLUDE for headers
set "INCLUDE=%MSVC_ROOT%\include;%WINSDK_INCLUDE%\um;%WINSDK_INCLUDE%\ucrt;%WINSDK_INCLUDE%\shared"

REM Ensure MSVC link.exe is found first
set "PATH=%MSVC_ROOT%\bin\Hostx64\x64;%MSVC_ROOT%\bin\Hostx86\x64;%PATH%"

REM Run cargo with the MSVC target
cargo %*
