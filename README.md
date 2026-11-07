# VideoPipeline

VideoPipeline kompresuje filmy do HEVC albo AV1 na karcie graficznej: NVIDIA (NVENC) albo Intel (QSV, zapasowo VAAPI).
Ma interfejs terminalowy w stylu Midnight Commandera, a podany z plikami działa jako zwykłe polecenie do skryptów.

![Panele: pliki i kolejka kodowania](docs/screenshots/panele.png)

## Do czego służy

- **Kompresja nagrań i filmów** — materiał z aparatu, kamery czy płyt trafia do HEVC/AV1 i zajmuje kilka razy mniej miejsca.
- **Zachowanie HDR** — HDR10 przechodzi bez zmian, a w HEVC także Dolby Vision (RPU) i HDR10+ (dynamiczne metadane).
- **Porządkowanie ścieżek** — domyślnie zostaje tylko polskie audio; opcjonalnie angielskie audio i napisy PL/EN,
  polska ścieżka może zostać przekodowana do E-AC-3 192 kb/s.
- **Kadrowanie** — automatyczne wykrywanie i usuwanie czarnych pasów (cropdetect).
- **Praca wsadowa** — kolejka wielu plików, dwa kodowania naraz, wznawianie przerwanej pracy.
- **Bezpieczne kopiowanie** — opcjonalna kopia wyniku obok źródła z weryfikacją SHA-512 (do 3 prób).

## Wymagany sprzęt i system

| Element | Wymaganie |
|---|---|
| Procesor | x86-64-v3 (AVX2, BMI2, FMA): Intel Haswell (4. gen. Core) / AMD Zen 1 lub nowszy |
| System | Linux z glibc 2.41+ (np. CachyOS/Arch, Debian 13, Ubuntu 25.04) — dotyczy AppImage i deb |
| Karta NVIDIA | HEVC: GeForce GTX 10xx (Pascal) lub nowsza; AV1: RTX 40xx / 50xx. Sterownik 570 lub nowszy (`nvidia-smi` do monitora obciążenia) |
| Karta Intel | HEVC: Core 7. gen. (Kaby Lake) lub nowszy; AV1: Arc (A/B), Core Ultra (Meteor Lake) lub nowszy |
| Sterowniki Intel | `intel-media-driver` (iHD, VA-API) i runtime oneVPL: `vpl-gpu-rt` (Tiger Lake i nowsze) albo `intel-media-sdk` (starsze); na Debianie `intel-media-va-driver-non-free` i `libmfx-gen1.2`. Użytkownik musi mieć dostęp do `/dev/dri/renderD*` (grupa `render`) |
| Miejsce na dysku | wolne miejsce w katalogu wyjściowym na pliki tymczasowe (rzędu rozmiaru wyniku) |

Wystarczy jedna karta. Gdy w komputerze są obie, domyślnie koduje NVIDIA, a Intela wybiera się jawnie (niżej).

## Instalacja

Gotowe pliki są na stronie [Releases](https://github.com/8brzrs26ws-cloud/videopipeline/releases). Oba warianty zawierają
własne ffmpeg i ffprobe (z NVENC, QSV i VAAPI), mkvmerge, dovi_tool i hdr10plus_tool; sterowniki GPU pochodzą z systemu.

**AppImage** (dowolna dystrybucja spełniająca wymagania):

    chmod +x VideoPipeline-*-x86_64_v3.AppImage
    ./VideoPipeline-*-x86_64_v3.AppImage
    ./VideoPipeline-*-x86_64_v3.AppImage --appimage-extract-and-run   # gdy system nie ma FUSE

**Pakiet deb** (Debian 13, Ubuntu 25.04+) — instaluje się w `/opt/videopipeline`, dodaje polecenie `videopipeline`, wpis w menu i ikonę:

    sudo apt install ./videopipeline_*_amd64.deb

Na Archu/CachyOS najprościej użyć AppImage. Sumy kontrolne leżą obok plików (`.sha256`).

## Użycie

### Interfejs (TUI)

Uruchomione bez argumentów (albo dwuklikiem — program sam otworzy okno terminala) pokazuje dwa panele:
po lewej pliki, po prawej kolejkę kodowania.

| Klawisz | Działanie |
|---|---|
| Tab | przełącz panel (pliki ↔ kolejka) |
| Enter / Backspace | wejdź do katalogu (lub dodaj plik) / katalog wyżej |
| Spacja, Ins | zaznacz / odznacz |
| Shift+↑↓ / PgUp / PgDn / Home / End | zaznaczanie wielu pozycji |
| `+` `-` `*` | zaznacz wszystko / wyczyść / odwróć |
| F5 | dodaj zaznaczone do kolejki (katalog = wszystkie filmy w nim) |
| F7 | opcje |
| F8 / Del | usuń z kolejki |
| F9 | koduj (Esc przerywa) |
| F10, Ctrl+C | wyjście |

**F7 — opcje.** Wiersz „Kodek wideo” przełącza między działającymi w systemie kombinacjami kodeka i karty,
np. `HEVC NVENC → AV1 NVENC → HEVC QSV`; pod listą widać, co wykryto. Pozostałe wiersze odpowiadają flagom z tabeli niżej.

![Okno opcji F7](docs/screenshots/opcje-f7.png)

**F9 — kodowanie.** Okno pokazuje postęp każdego pliku, prędkość, ETA, log i obciążenie sprzętu:

- NVIDIA: GPU, NVENC (koder), NVDEC (dekoder), VRAM — z `nvidia-smi`;
- Intel: VIDEO (blok VDBox, który sprzętowo koduje i dekoduje — stąd jedna wartość), FILTRY (kadrowanie, odszumianie
  i konwersja formatu; sterownik wykonuje je na VEBox albo na jednostkach EU, więc pokazane jest większe z obciążeń),
  taktowanie i pamięć karty — z `/proc/<pid>/fdinfo` procesów ffmpeg, bez uprawnień roota;
- zawsze CPU i RAM.

![Okno kodowania — NVIDIA NVENC](docs/screenshots/kodowanie.png)

Kodowanie na karcie Intel (HEVC QSV, Iris Xe w Core i5-1235U, dekodowanie `h264_qsv`):

![Okno kodowania — Intel QSV](docs/screenshots/kodowanie-intel.png)

### Tryb skryptowy

Podanie plików lub katalogów pomija TUI: wszystko trafia do kolejki i kodowanie startuje od razu, z postępem
wypisywanym jako tekst.

    videopipeline film.mkv katalog/            # HEVC na domyślnej karcie
    videopipeline -av1 -c katalog/             # AV1 + kopia do <źródło>/encoded z weryfikacją SHA-512
    videopipeline --intel -q 22 film.mp4       # HEVC na karcie Intel, CQ 22
    videopipeline -b                           # bieżący katalog
    videopipeline --detect                     # pokaż wykryte enkodery i zakończ

Kod wyjścia: 0 — sukces, 1 — błędy, 130 — przerwano (Ctrl+C). `--tui` z plikami otwiera TUI z wypełnioną kolejką.

### Wybór karty

Przy starcie program wykrywa enkodery krótkim kodowaniem testowym (sama obecność w ffmpeg nic nie znaczy).

- Tylko NVIDIA albo tylko Intel — używana jest ta karta, bez żadnych flag.
- Obie karty — domyślnie NVIDIA; Intel: `--intel` (QSV, a gdy nie działa — VAAPI) albo `--engine qsv|vaapi`.
- Na karcie Intel QSV dekoduje źródło sprzętowo (H.264, HEVC, AV1, VP9, MPEG-2, VC-1); kodek bez wsparcia sprzętowego
  jest dekodowany na CPU. VAAPI dekoduje przez VA-API.

### Opcje

| Flaga | Działanie |
|---|---|
| `-av1` | AV1 zamiast HEVC (zawsze 10-bit; bez Dolby Vision i HDR10+) |
| `--intel`, `--engine nvenc\|qsv\|vaapi` | wybór karty (wyżej) |
| `-q N`, `--hevc-cq N`, `--av1-cq N` | jakość, 1–51, mniej = lepiej (domyślnie HEVC 19, AV1 25) |
| `--qsv-preset P` | preset Intel QSV: `veryfast` … `veryslow` (domyślnie `slow`); `veryslow` na zintegrowanej grafice bywa kilka razy wolniejszy przy niewielkim zysku jakości |
| `-pleac3` | polska ścieżka → E-AC-3 stereo 192 kb/s |
| `-da` | dodatkowo angielskie audio (E-AC-3 5.1) oraz napisy EN i PL |
| `-d` | łagodne odszumianie (tylko źródła SDR 8-bit; CUDA, vpp_qsv / denoise_vaapi albo CPU) |
| `-pc` | bez wykrywania czarnych pasów (najdłuższy etap analizy) |
| `-pdv`, `-ph` | bez Dolby Vision / bez HDR10+ |
| `-c` | kopia wyniku do `<katalog źródła>/encoded` z weryfikacją SHA-512 |
| `-s` | wyłącz komputer po zakończeniu |
| `-b` | tryb skryptowy dla bieżącego katalogu |

**Jakość.** Ta sama wartość CQ trafia do każdej karty: NVENC `-cq`, QSV `-global_quality` (ICQ), VAAPI `-qp` (CQP;
dla AV1 przeliczone ×5 na skalę 0–255). Skale enkoderów nie są identyczne, więc ten sam CQ na różnych kartach daje
podobną, ale nie taką samą jakość i rozmiar.

**Pliki wynikowe** trafiają do `~/Wideo` (zmiana w F7). Kontener wynika ze źródła: `.mp4` → `.mp4`, wszystko inne → `.mkv`.
Nazwa zawiera kodek i opcje, np. `film_hevc.mkv`, `film_hevc_qsv_cq22.mkv`, `film_av1_10bit_cq23_da.mkv`, więc wyniki
z różnymi ustawieniami się nie nadpisują. Przerwane kodowanie wznawia się od ostatniego ukończonego etapu.
W MP4 Dolby Vision zostaje w strumieniu, ale bez rekordu konfiguracji — część odtwarzaczy pokaże wtedy zwykły HDR10.

## Budowanie ze źródeł

### Wersja deweloperska

Potrzebne: Rust (stable) oraz w `PATH`: ffmpeg i ffprobe z obsługą wybranej karty (NVENC/CUDA albo QSV/VAAPI),
mkvmerge (mkvtoolnix), [dovi_tool](https://github.com/quietvoid/dovi_tool), [hdr10plus_tool](https://github.com/quietvoid/hdr10plus_tool).

    ./build.sh       # target/release/videopipeline + libvideopipeline_core.so
    ./install.sh     # opcjonalnie: ~/.local/bin, ikona i wpis w menu

Program to biblioteka z całą logiką (`core/`) i binarka z interfejsem (`app/`). Oba pliki muszą pochodzić z jednego builda
i leżeć w tym samym katalogu (albo binarka w `bin/`, biblioteka w `../lib/`).

### AppImage i deb (Docker)

Wydania buduje się w kontenerze Debian 13, więc wynik nie zależy od systemu, na którym budujesz:

    ./install-build-deps.sh     # raz: Docker, git, gh (Arch/CachyOS, Debian/Ubuntu); potem wyloguj się i zaloguj
    ./build-local.sh            # dist/: AppImage, deb i sumy .sha256; cache w ./build
    ./release.sh vX.Y.Z         # wysyła dist/ do GitHub Releases (wymaga gh auth login)

Zmienne dla `build-local.sh`:

| Zmienna | Znaczenie |
|---|---|
| `FFMPEG_VERSION` | wersja ffmpeg, domyślnie `latest` (najnowsze wydanie) |
| `DOVI_TOOL_REF`, `HDR10PLUS_TOOL_REF` | tagi narzędzi (domyślnie przypięte), `latest` = najnowsze |
| `INTEL` | `auto` (domyślnie), `1` — wymagaj QSV/VAAPI, `0` — bez Intela |
| `TARGET_CPU` | domyślnie `x86-64-v3` |
| `FORCE=1` | przebuduj ffmpeg i narzędzia mimo cache |
| `DOCKER=podman` | Podman zamiast Dockera |

To samo robi ręcznie uruchamiany workflow GitHub Actions (`.github/workflows/release.yml`).
Licencje dołączonych składników: [`packaging/appimage/THIRD-PARTY-NOTICES.md`](packaging/appimage/THIRD-PARTY-NOTICES.md).

## Licencja

GNU GPL v3 — zob. [`LICENSE`](LICENSE).
