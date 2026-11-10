@echo off
rem Tworzy skrot "VideoPipeline" na pulpicie i w menu Start (okno z ikona VideoPipeline
rem na pasku zadan: dolaczony terminal videopipeline-term.exe albo konsola conhost).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0utworz-skrot.ps1"
pause
