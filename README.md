# VideoPipeline v11

Port skryptu fish V9 na Rusta: biblioteka dynamiczna z całą logiką + binarka z interfejsem.

```
videopipeline-v11/
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

## Jakość (CQ)
Domyślnie HEVC CQ 19, AV1 CQ 25 (mniej = lepsza jakość i większy plik). Zmiana w F7 (←/→ ±1, PgUp/PgDn ±5)
albo flagami `--hevc-cq N`, `--av1-cq N`, `-q N` (dla wybranego kodeka). Zakres 1–51.
Przy wartości innej niż domyślna nazwy plików dostają `_cqN` (np. `film_hevc_cq16.mkv`), więc wyniki o różnej
jakości nie nadpisują się, a wznowienie nie użyje tymczasowego pliku z innym CQ.

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
