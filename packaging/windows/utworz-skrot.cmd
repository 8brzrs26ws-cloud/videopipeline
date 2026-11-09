@echo off
rem Tworzy skrot "VideoPipeline" na pulpicie i w menu Start. Skrot uruchamia program
rem w klasycznej konsoli (conhost), wiec pasek zadan pokazuje ikone VideoPipeline.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0utworz-skrot.ps1"
pause
