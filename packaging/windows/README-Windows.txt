VideoPipeline dla Windows (x64)
================================

Uruchamianie
  Rozpakuj cały folder w dowolnym miejscu i uruchom videopipeline.exe
  (dwuklik otwiera okno konsoli). Najlepiej wygląda w Windows Terminal.
  Wszystkie pliki muszą zostać w jednym folderze: program używa dołączonych
  ffmpeg.exe, ffprobe.exe, mkvmerge.exe, MediaInfo.exe, dovi_tool.exe,
  hdr10plus_tool.exe, mpv.exe (odtwarzanie pod F3) i videopipeline-term.exe (okno programu).

  Tryb skryptowy (PowerShell / cmd):
    .\videopipeline.exe film.mkv D:\Filmy\
    .\videopipeline.exe --intel -q 22 film.mp4
    .\videopipeline.exe --detect

Wymagania
  - Windows 10 lub 11, 64-bit
  - procesor x86-64-v3 (AVX2): Intel Haswell / AMD Zen 1 lub nowszy
  - NVIDIA: GeForce GTX 10xx lub nowsza (AV1: RTX 40xx/50xx), aktualny sterownik
  - Intel: Core 7. gen. lub nowszy (AV1: Arc, Core Ultra), aktualny sterownik grafiki Intel
    (kodowanie przez Quick Sync / oneVPL; VAAPI jest tylko w wersji dla Linuksa)

Uwagi
  - Pliki nie są podpisane cyfrowo — SmartScreen może poprosić o potwierdzenie
    („Więcej informacji” → „Uruchom mimo to”).
  - Domyślny katalog wyników: %USERPROFILE%\Videos (zmiana w F7).
  - videopipeline.exe uruchomiony dwuklikiem otwiera się w dołączonym terminalu
    videopipeline-term.exe (Alacritty) z ikoną VideoPipeline na pasku zadań.
    Gdy ten terminal nie wystartuje (brak OpenGL, np. Pulpit zdalny), program
    działa w zwykłej konsoli Windows.
  - utworz-skrot.cmd tworzy skrót "VideoPipeline" na pulpicie i w menu Start;
    skrót można przypiąć do paska zadań.
  - F3 otwiera plik w dołączonym mpv w osobnym oknie (q zamyka odtwarzacz).
  - W lewym panelu, w katalogu głównym dysku, widać pozostałe dyski (C:, D: …).

Licencja: GNU GPL v3 (LICENSE.txt). Licencje dołączonych programów: THIRD-PARTY-NOTICES.md.
