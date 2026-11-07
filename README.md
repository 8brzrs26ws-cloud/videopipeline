# VideoPipeline 0.11.1

Port skryptu fish V9 na Rusta: biblioteka dynamiczna z całą logiką + binarka z interfejsem.

```
videopipeline/
├── core/   → libvideopipeline_core.so  (analiza, NVENC, mux, kopiowanie/SHA-512, monitoring)
└── app/    → videopipeline             (TUI w stylu Midnight Commandera + tryb skryptowy)
```

## Budowanie
    ./build.sh            # = cargo build --release z -C prefer-dynamic i rpath (biblioteka + libstd)
    cargo run --release   # też działa (cargo ustawia LD_LIBRARY_PATH)

Rust nie ma stabilnego ABI: `.so` i binarka muszą pochodzić z jednego builda tym samym `rustc`.
Po skopiowaniu trzymaj `videopipeline` i `libvideopipeline_core.so` w jednym katalogu
(albo binarkę w `bin/`, a bibliotekę w `../lib/`). `libstd` jest szukany w sysroot rustc.

Wymaga w PATH: ffmpeg, ffprobe, mkvmerge, dovi_tool, hdr10plus_tool, nvidia-smi (monitor GPU; opcjonalny), `shutdown` dla -s.

## Dwa tryby
| Wywołanie | Efekt |
|---|---|
| `videopipeline` (bez ścieżek) | TUI; flagi ustawiają początkowe opcje |
| `videopipeline film.mkv katalog/ -av1 -c` | **tryb skryptowy**: pliki/katalogi → kolejka → od razu kodowanie, bez TUI (sam tekst, mało RAM-u) |
| `videopipeline -b` | tryb skryptowy dla katalogu bieżącego (jak V9 bez argumentu) |
| `videopipeline --tui film.mkv` | wymuszone TUI, ścieżki trafiają do kolejki |

Kod wyjścia trybu skryptowego: 0 = OK, 1 = błędy, 130 = przerwano (Ctrl+C).

## Opcje (flagi = opcje w oknie F7)
`-av1`, `-pleac3`, `-da`, `-d`, `-c`, `-s` — jak w V9, oraz nowe:
- `-pc` (`-nocrop`) — pomija cropdetect (najdłuższy etap analizy),
- `-pdv` (`-nodv`) — pomija analizę/ekstrakcję/wstrzykiwanie Dolby Vision,
- `-ph` (`-nohdr10plus`) — pomija analizę/ekstrakcję/wstrzykiwanie HDR10+.

Zmiana tych opcji po wcześniejszej analizie powoduje ponowną analizę (o ile nie ma jeszcze zakodowanego wideo).
Analiza ffprobe (ścieżki audio/napisy, profil koloru) jest zawsze wykonywana — jest szybka i potrzebna do muxu.

## Kontener wyjściowy
Plik wejściowy `.mp4`/`.MP4` daje wynik `.mp4`, każdy inny (mkv, mov, avi, m2ts) wynik `.mkv`.
MKV składa mkvmerge, MP4 — ffmpeg (`-c copy`, `-tag:v hvc1`, `+faststart`; zapis do `*.mp4.part`, potem zmiana nazwy,
żeby przerwany mux nie wyglądał na gotowy wynik). Audio i napisy źródła są kopiowane bez zmian (źródło MP4 ma je
w formatach zgodnych z MP4), polskie/angielskie E-AC-3 z `-pleac3`/`-da` dostają jawnie ustawiony język.
Ograniczenie MP4: RPU Dolby Vision i metadane HDR10+ zostają w strumieniu, ale ffmpeg nie zapisuje konfiguracji DV
(dvcC), więc część odtwarzaczy pokaże zwykły HDR10. Gdy zależy Ci na pełnym DV, użyj źródła w MKV.

## Jakość (CQ)
Domyślnie HEVC CQ 19, AV1 CQ 25 (mniej = lepsza jakość i większy plik). Zmiana w F7 (←/→ ±1, PgUp/PgDn ±5)
albo flagami `--hevc-cq N`, `--av1-cq N`, `-q N` (dla wybranego kodeka). Zakres 1–51.
Przy wartości innej niż domyślna nazwy plików dostają `_cqN` (np. `film_hevc_cq16.mkv`), więc wyniki o różnej
jakości nie nadpisują się, a wznowienie nie użyje tymczasowego pliku z innym CQ.

## Wydania: AppImage i deb (x86-64-v3)
Na stronie *Releases* na GitHubie jest gotowy plik `VideoPipeline-<wersja>-x86_64_v3.AppImage` z dołączonymi:
ffmpeg i ffprobe (NVENC/NVDEC/CUDA, statycznie), mkvmerge, dovi_tool, hdr10plus_tool oraz bibliotekami programu.
Kod własny i narzędzia z Rusta/ffmpeg są kompilowane pod **x86-64-v3** (AVX2, BMI2, FMA — Haswell/Zen 1 i nowsze);
na starszym procesorze AppImage wypisze komunikat i nie ruszy. Sterowniki NVIDIA i `nvidia-smi` pochodzą z systemu.

    chmod +x VideoPipeline-*-x86_64_v3.AppImage
    ./VideoPipeline-*-x86_64_v3.AppImage            # TUI; z plikami/katalogami: tryb skryptowy
    ./VideoPipeline-*.AppImage --appimage-extract-and-run   # gdy brakuje FUSE (libfuse2)

Dwuklik na pliku AppImage w KDE otwiera okno terminala (patrz niżej). Sumy kontrolne: plik `.sha256` obok obrazu.

### Pakiet deb
`videopipeline_<wersja>_amd64.deb` instaluje się w `/opt/videopipeline` (z własnymi ffmpeg, mkvmerge, dovi_tool, hdr10plus_tool),
dodaje polecenie `videopipeline`, wpis w menu i ikony. Zależności: tylko `libc6 (>= 2.41)`, `libstdc++6`, `libgcc-s1`
(Debian 13 lub nowszy); sterownik GPU zostaje w systemie.

    sudo apt install ./videopipeline_*_amd64.deb

Na Archu/CachyOS: `bsdtar -xf videopipeline_*.deb data.tar.xz && sudo tar -C / -xf data.tar.xz` albo użyj AppImage.

### Budowanie wydań (lokalnie, Docker)
Wydania budujemy w kontenerze Debian 13 (glibc 2.41), więc wynik nie zależy od systemu hosta:

    ./build-local.sh                  # dist/: AppImage, deb, sumy .sha256 (cache w ./build — kolejne budowania są szybkie)
    ./release.sh v0.11.1              # wysyła dist/ na GitHub (gh auth login); tworzy tag, jeśli go nie ma

Zmienne środowiskowe: `FFMPEG_VERSION`, `DOVI_TOOL_REF`, `HDR10PLUS_TOOL_REF` (wartość `latest` = najnowsze wydanie; domyślnie
przypięte: ffmpeg 7.1.1, dovi_tool 2.3.4, hdr10plus_tool 1.7.2), `NVCODEC_HEADERS_REF` (domyślnie master),
`INTEL=auto|1|0` (ffmpeg z VAAPI i QSV/libvpl dla przyszłego kodowania na Intelu; `auto` — jeśli są statyczne libva/libvpl),
`TARGET_CPU` (domyślnie x86-64-v3), `FORCE=1` (przebuduj składniki), `DOCKER=podman`.
Workflow `.github/workflows/release.yml` robi to samo na GitHubie (ręcznie: *Actions → Run workflow*, lub po tagu `v*`).
Licencje dołączonych składników: `packaging/appimage/THIRD-PARTY-NOTICES.md`.

## Uruchamianie z menu / dwuklikiem (KDE)
`./install.sh` (po `./build.sh`) kopiuje program do `~/.local/bin`, instaluje ikonę (`icons/`, klatka filmu w imadle)
i dodaje wpis „VideoPipeline” do menu aplikacji (`Terminal=true`). Dwuklik bezpośrednio na pliku binarnym też działa:
bez terminala program sam otwiera okno (konsole, gnome-terminal, xfce4-terminal, kitty, alacritty, wezterm, foot, xterm).
Własny terminal: `VIDEOPIPELINE_TERMINAL="konsole -e"`. Tryb skryptowy (podane pliki) nie otwiera okna.

## Kodowanie
Oba równoległe enkodery NVENC dekodują przez CUDA/NVDEC (`-hwaccel cuda`).

## TUI
| Klawisz | Działanie |
|---|---|
| Tab | przełącz panel (pliki ↔ kolejka) |
| Enter / Backspace | wejdź do katalogu (lub dodaj plik) / katalog wyżej |
| Spacja, Ins | zaznacz / odznacz |
| **Shift+↑↓ / PgUp / PgDn / Home / End** | zaznaczanie wielu pozycji (jak w MC) |
| `+` `-` `*` | zaznacz wszystko / wyczyść / odwróć |
| F5 | dodaj zaznaczone (lub bieżący; katalog = wszystkie filmy) do kolejki |
| F7 | okno opcji |
| F8 / Del | usuń z kolejki |
| F9 | koduj; Esc przerywa |
| F10, Ctrl+C | wyjście |

Okno kodowania pokazuje m.in. obciążenie: GPU, NVENC, NVDEC, VRAM (z `nvidia-smi -l 1`, jeden proces
na cały czas kodowania), CPU i RAM (z /proc). Shift+strzałki wymagają terminala wysyłającego modyfikatory
(kitty, alacritty, gnome-terminal, konsole itd. — tak).

## Pozostałe
- Stan analizy: `*.analysis.json` (stare `.env` z fisha nie są wczytywane).
- Audio: domyślnie tylko polska ścieżka; `-da` dodaje audio EN (E-AC-3 5.1) i napisy EN/PL (suffix `_da`).
- Kopiowanie `-c`: SHA-512, do 3 prób; po porażce kopia jest kasowana i powstaje `encoded/file_transfer_error`.
- AV1: w nazwach "cq23", ale realny parametr to `-cq 25` (stała `AV1_CQ` w `core/src/config.rs`).

## Licencja
GNU GPL v3 — zob. plik `LICENSE`.
