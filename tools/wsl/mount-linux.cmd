@echo off
rem Montuje w WSL partycje Linuksa z dyskow tego komputera (szczegoly: mount-linux.ps1).
rem   mount-linux.cmd            montowanie
rem   mount-linux.cmd -Unmount   odmontowanie
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0mount-linux.ps1" %*
