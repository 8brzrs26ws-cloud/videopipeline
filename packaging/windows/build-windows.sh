#!/usr/bin/env bash
# Kroskompilacja wersji Windows x64 (x86_64-pc-windows-gnu, MinGW-w64) w kontenerze Debian 13:
#   dist/VideoPipeline-<wersja>-windows-x86_64_v3.zip (+ .sha256)
# Zawartość: videopipeline.exe + videopipeline_core.dll + std-*.dll, ffmpeg.exe i ffprobe.exe
# (statyczne: NVENC/NVDEC/CUDA-LLVM, Intel QSV przez oneVPL + D3D11, dav1d, zlib), mkvmerge.exe
# (oficjalna wersja przenośna MKVToolNix), MediaInfo.exe (oficjalne CLI MediaArea), dovi_tool.exe, hdr10plus_tool.exe.
# Sterowniki GPU (NVIDIA, Intel) pochodzą z systemu Windows.
#
# Zmienne: VERSION, FFMPEG_VERSION (latest = najnowsze wydanie), DOVI_TOOL_REF, HDR10PLUS_TOOL_REF,
# DAV1D_REF, LIBVPL_REF, MKVTOOLNIX_VERSION, MEDIAINFO_VERSION (latest = najnowsze), NVCODEC_HEADERS_REF, TARGET_CPU, JOBS, FORCE=1.
# Uruchamiane przez ./build-local.sh (TARGETS zawiera "windows").
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="${VERSION:-$(grep -m1 '^version' app/Cargo.toml | cut -d'"' -f2)}"
FFMPEG_VERSION="${FFMPEG_VERSION:-latest}"
DOVI_TOOL_REF="${DOVI_TOOL_REF:-2.3.4}"
HDR10PLUS_TOOL_REF="${HDR10PLUS_TOOL_REF:-1.7.2}"
DAV1D_REF="${DAV1D_REF:-latest}"
LIBVPL_REF="${LIBVPL_REF:-latest}"
MKVTOOLNIX_VERSION="${MKVTOOLNIX_VERSION:-latest}"
MEDIAINFO_VERSION="${MEDIAINFO_VERSION:-latest}"
# mpv (F3): gotowa kompilacja shinchiro/mpv-winbuild-cmake (tag = data, np. 20261108); "latest" = najnowsza.
MPV_WIN_TAG="${MPV_WIN_TAG:-latest}"
# Terminal dołączony do paczki: Alacritty (tag vX.Y.Z) z ikoną VideoPipeline, jako videopipeline-term.exe.
ALACRITTY_VERSION="${ALACRITTY_VERSION:-latest}"
NVCODEC_HEADERS_REF="${NVCODEC_HEADERS_REF:-}"
TARGET_CPU="${TARGET_CPU:-x86-64-v3}"
JOBS="${JOBS:-$(nproc)}"
FORCE="${FORCE:-0}"

TRIPLE=x86_64-w64-mingw32
RTARGET=x86_64-pc-windows-gnu
# Wariant "posix" MinGW (wątki C++ dla libvpl); -static wciąga libwinpthread do plików .exe.
CC_W="$TRIPLE-gcc-posix"
CXX_W="$TRIPLE-g++-posix"

BUILD="$ROOT/build"
CACHE="$BUILD/cache/win64"   # trwałe: zależności statyczne, ffmpeg, narzędzia, MKVToolNix
WORK="$BUILD/work/win64"
PKGNAME="VideoPipeline-$VERSION-windows-x86_64_v3"
PKG="$WORK/$PKGNAME"
DIST="$ROOT/dist"
DEPS="$CACHE/deps"           # prefiks bibliotek statycznych (dav1d, libvpl, nv-codec-headers)
mkdir -p "$CACHE" "$WORK" "$DIST" "$DEPS"

log()  { printf '\n\033[1;36m==> [Windows] %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mBŁĄD [Windows]: %s\033[0m\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "brak polecenia: $1"; }
# Każdy nieobsłużony błąd (set -e) pokazuje, gdzie i na czym skrypt się zatrzymał.
trap 'fail "przerwano w linii $LINENO: $BASH_COMMAND"' ERR

for c in cargo rustc "$CC_W" "$CXX_W" "$TRIPLE-objdump" "$TRIPLE-strip" "$TRIPLE-windres" meson ninja cmake nasm clang \
         pkg-config git wget zip; do need "$c"; done
SEVENZ="$(command -v 7zz || command -v 7z || true)"
[[ -n "$SEVENZ" ]] || fail "brak 7zz/7z (pakiet 7zip)"
rustup target list --installed 2>/dev/null | grep -qx "$RTARGET" \
    || fail "brak celu Rust $RTARGET (rustup target add $RTARGET — robi to docker/Dockerfile)"

# ------------------------------------------------------------------ wersje "latest"
# Każde ustalenie wersji jest odporne na pipefail i kończy się czytelnym błędem (wcześniej skrypt
# potrafił zakończyć się bez komunikatu, gdy np. strona z listą wersji nie odpowiedziała).
export GIT_TERMINAL_PROMPT=0   # git ls-remote nie może pytać o login (repozytorium niedostępne = brak wyniku)
latest_git_tag() {   # $1 = repo, $2 = opcjonalny prefiks tagu (np. "release-"); najwyższy tag semver
    local pre="${2:-}"
    { timeout 60 git ls-remote --tags --refs "$1" 2>/dev/null || true; } | sed 's|.*refs/tags/||' \
        | { grep -E "^${pre}v?[0-9]+(\.[0-9]+)+\$" || true; } | sed -E "s/^${pre}v?//" | sort -V | tail -n1
}
resolve() {   # $1 = nazwa zmiennej, $2 = opis źródła, reszta = polecenie wypisujące wersję
    local name="$1" what="$2"; shift 2
    [[ "${!name}" = latest ]] || return 0
    local v; v="$("$@" || true)"
    [[ -n "$v" ]] || fail "nie udało się ustalić najnowszej wersji: $name ($what) — podaj ją ręcznie, np. $name=..."
    printf -v "$name" '%s' "$v"
}
# shellcheck source=../ffmpeg-source.sh
source "$ROOT/packaging/ffmpeg-source.sh"
resolve FFMPEG_VERSION "ffmpeg.org, GitHub, git.ffmpeg.org" ffmpeg_latest "$BUILD/work"
resolve DOVI_TOOL_REF "github.com/quietvoid/dovi_tool" latest_git_tag https://github.com/quietvoid/dovi_tool
resolve HDR10PLUS_TOOL_REF "github.com/quietvoid/hdr10plus_tool" latest_git_tag https://github.com/quietvoid/hdr10plus_tool
resolve DAV1D_REF "code.videolan.org/videolan/dav1d" latest_git_tag https://code.videolan.org/videolan/dav1d.git
# libvpl: tylko wydania 2.x — repozytorium ma też stare tagi oneVPL "v2023.x", które sortują się wyżej.
libvpl_latest() {
    { git ls-remote --tags --refs https://github.com/intel/libvpl.git 2>/dev/null || true; } | sed 's|.*refs/tags/||' \
        | { grep -E '^v2\.[0-9]+\.[0-9]+$' || true; } | sed 's/^v//' | sort -V | tail -n1
}
resolve LIBVPL_REF "github.com/intel/libvpl" libvpl_latest
# MKVToolNix — kilka źródeł po kolei, bo pojedyncze bywają niedostępne:
# plik sprawdzania aktualizacji z mkvtoolnix.download, tagi "release-X.Y" (Codeberg, GitLab),
# lista wydań dla Windows, a na końcu wersja już pobrana do cache.
mkvtoolnix_latest() {
    local v=""
    v="$({ wget -qO- --timeout=20 https://mkvtoolnix.download/latest-release.xml.gz || true; } \
        | { gzip -dc 2>/dev/null || true; } | { grep -oE '<version>[0-9]+\.[0-9]+(\.[0-9]+)?</version>' || true; } \
        | sed -E 's|</?version>||g' | sort -V | tail -n1)"
    [[ -n "$v" ]] || v="$(latest_git_tag https://codeberg.org/mbunkus/mkvtoolnix.git release-)"
    [[ -n "$v" ]] || v="$(latest_git_tag https://gitlab.com/mbunkus/mkvtoolnix.git release-)"
    [[ -n "$v" ]] || v="$({ wget -qO- --timeout=20 https://mkvtoolnix.download/windows/releases/ || true; } \
        | { grep -oE 'href="[0-9]+\.[0-9]+(\.[0-9]+)?/"' || true; } | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | sort -uV | tail -n1)"
    if [[ -z "$v" ]]; then
        v="$(ls -1d "$CACHE"/mkvtoolnix-* 2>/dev/null | sed 's|.*/mkvtoolnix-||' | sort -V | tail -n1)"
        [[ -n "$v" ]] && echo "UWAGA: źródła wersji MKVToolNix niedostępne — używam pobranej wcześniej $v" >&2
    fi
    echo "$v"
}
resolve MKVTOOLNIX_VERSION "tagi release-* MKVToolNix" mkvtoolnix_latest
# MediaInfo: tagi vRR.MM w repozytorium MediaArea; zapasowo wersja pobrana wcześniej do cache.
mediainfo_latest() {
    local v; v="$(latest_git_tag https://github.com/MediaArea/MediaInfo.git)"
    [[ -n "$v" ]] || v="$(ls -1d "$CACHE"/mediainfo-* 2>/dev/null | sed 's|.*/mediainfo-||' | sort -V | tail -n1)"
    echo "$v"
}
resolve MEDIAINFO_VERSION "github.com/MediaArea/MediaInfo" mediainfo_latest
# mpv dla Windows: najnowsze wydanie shinchiro (API GitHuba), zapasowo wersja pobrana wcześniej do cache.
mpv_win_latest() {
    local v; v="$({ wget -qO- --timeout=20 --tries=2 https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest || true; } \
        | { grep -oE '"tag_name": *"[^"]+"' || true; } | head -n1 | sed -E 's/.*"([^"]+)"$/\1/')"
    [[ -n "$v" ]] || v="$(latest_git_tag https://github.com/shinchiro/mpv-winbuild-cmake.git)"
    if [[ -z "$v" ]]; then
        v="$(ls -1d "$CACHE"/mpv-win-* 2>/dev/null | sed 's|.*/mpv-win-||' | sort -V | tail -n1)"
        [[ -n "$v" ]] && echo "UWAGA: GitHub niedostępny — używam pobranego wcześniej mpv $v" >&2
    fi
    echo "$v"
}
resolve MPV_WIN_TAG "github.com/shinchiro/mpv-winbuild-cmake" mpv_win_latest
alacritty_latest() {
    local v; v="$(latest_git_tag https://github.com/alacritty/alacritty.git)"
    [[ -n "$v" ]] || v="$(ls -1d "$CACHE"/alacritty-* 2>/dev/null | sed -E 's|.*/alacritty-([0-9.]+)-.*|\1|' | sort -V | tail -n1)"
    echo "$v"
}
resolve ALACRITTY_VERSION "github.com/alacritty/alacritty" alacritty_latest
echo "Wersje: ffmpeg $FFMPEG_VERSION, dav1d $DAV1D_REF, libvpl $LIBVPL_REF, MKVToolNix $MKVTOOLNIX_VERSION, MediaInfo $MEDIAINFO_VERSION, mpv $MPV_WIN_TAG, Alacritty $ALACRITTY_VERSION," \
     "dovi_tool $DOVI_TOOL_REF, hdr10plus_tool $HDR10PLUS_TOOL_REF"

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$TRIPLE-gcc"

# ------------------------------------------------------------------ 1. aplikacja
log "1/6 videopipeline.exe + videopipeline_core.dll ($TARGET_CPU)"
# RUSTFLAGS zastępuje .cargo/config.toml — prefer-dynamic podajemy jawnie (biblioteka core jako DLL).
# app/build.rs wkompilowuje ikonę i informacje o wersji przez windres.
WINDRES="$TRIPLE-windres" RUSTFLAGS="-C prefer-dynamic -C target-cpu=$TARGET_CPU" cargo build --release --target "$RTARGET"
OUT="${CARGO_TARGET_DIR:-target}/$RTARGET/release"
[[ -f "$OUT/videopipeline.exe" && -f "$OUT/videopipeline_core.dll" ]] || fail "brak videopipeline.exe / videopipeline_core.dll"
STD_DLL="$(find "$(rustc --print sysroot)/lib/rustlib/$RTARGET" -name 'std-*.dll' | head -n1)"
[[ -n "$STD_DLL" ]] || fail "brak std-*.dll celu $RTARGET"

# ------------------------------------------------------------------ 2. dovi_tool, hdr10plus_tool
log "2/6 dovi_tool $DOVI_TOOL_REF + hdr10plus_tool $HDR10PLUS_TOOL_REF (.exe)"
TOOLS="$CACHE/tools"
STAMP="dovi=$DOVI_TOOL_REF hdr10plus=$HDR10PLUS_TOOL_REF cpu=$TARGET_CPU internal-font"
if [[ "$FORCE" = 1 || "$(cat "$TOOLS/.stamp" 2>/dev/null)" != "$STAMP" ]]; then
    export RUSTFLAGS="-C target-cpu=$TARGET_CPU"
    export CARGO_TARGET_DIR="$WORK/tools-target"
    for t in "dovi_tool:$DOVI_TOOL_REF" "hdr10plus_tool:$HDR10PLUS_TOOL_REF"; do
        cargo install --locked --force --root "$TOOLS" --target "$RTARGET" \
            --git "https://github.com/quietvoid/${t%%:*}" --tag "${t#*:}" \
            --no-default-features --features internal-font "${t%%:*}"
    done
    unset RUSTFLAGS CARGO_TARGET_DIR
    echo "$STAMP" > "$TOOLS/.stamp"
fi

# ------------------------------------------------------------------ 3. biblioteki statyczne
log "3/6 dav1d $DAV1D_REF, libvpl $LIBVPL_REF, nv-codec-headers (statycznie, MinGW)"
CROSSFILE="$WORK/meson-cross.txt"
cat > "$CROSSFILE" <<EOF
[binaries]
c = '$CC_W'
cpp = '$CXX_W'
ar = '$TRIPLE-ar'
strip = '$TRIPLE-strip'
windres = '$TRIPLE-windres'
pkg-config = 'pkg-config'
nasm = 'nasm'

[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF

DEPSTAMP="dav1d=$DAV1D_REF libvpl=$LIBVPL_REF nvh=${NVCODEC_HEADERS_REF:-master}"
if [[ "$FORCE" = 1 || "$(cat "$DEPS/.stamp" 2>/dev/null)" != "$DEPSTAMP" ]]; then
    rm -rf "$DEPS" && mkdir -p "$DEPS"

    rm -rf "$WORK/dav1d"
    git clone --quiet --depth 1 --branch "$DAV1D_REF" https://code.videolan.org/videolan/dav1d.git "$WORK/dav1d"
    meson setup "$WORK/dav1d/build" "$WORK/dav1d" --cross-file "$CROSSFILE" --prefix "$DEPS" --libdir lib \
        --buildtype release -Ddefault_library=static -Denable_tools=false -Denable_tests=false
    ninja -C "$WORK/dav1d/build" install

    rm -rf "$WORK/libvpl"
    git clone --quiet --depth 1 --branch "v$LIBVPL_REF" https://github.com/intel/libvpl.git "$WORK/libvpl"
    cmake -S "$WORK/libvpl" -B "$WORK/libvpl/build" -G Ninja \
        -DCMAKE_SYSTEM_NAME=Windows -DCMAKE_SYSTEM_PROCESSOR=x86_64 \
        -DCMAKE_C_COMPILER="$CC_W" -DCMAKE_CXX_COMPILER="$CXX_W" -DCMAKE_RC_COMPILER="$TRIPLE-windres" \
        -DCMAKE_FIND_ROOT_PATH="/usr/$TRIPLE" -DCMAKE_INSTALL_PREFIX="$DEPS" -DCMAKE_INSTALL_LIBDIR=lib \
        -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DBUILD_TESTS=OFF -DBUILD_EXAMPLES=OFF \
        -DBUILD_TOOLS=OFF -DINSTALL_EXAMPLES=OFF -DINSTALL_EXAMPLE_CODE=OFF -DENABLE_WARNING_AS_ERROR=OFF
    ninja -C "$WORK/libvpl/build" install
    # Statyczny dispatcher oneVPL jest w C++ i woła COM/rejestr — ffmpeg musi to dolinkować.
    VPC="$(find "$DEPS" -name vpl.pc | head -n1)"
    [[ -n "$VPC" ]] || fail "libvpl nie zainstalował vpl.pc"
    sed -i -E 's|^(Libs:.*)$|\1 -lstdc++ -lole32 -luuid -ladvapi32 -lsetupapi -lcfgmgr32|' "$VPC"

    rm -rf "$WORK/nv-codec-headers"
    git clone --quiet https://github.com/FFmpeg/nv-codec-headers.git "$WORK/nv-codec-headers"
    [[ -n "$NVCODEC_HEADERS_REF" ]] && git -C "$WORK/nv-codec-headers" checkout --quiet "$NVCODEC_HEADERS_REF"
    make -C "$WORK/nv-codec-headers" PREFIX="$DEPS" install >/dev/null

    echo "$DEPSTAMP" > "$DEPS/.stamp"
fi

# ------------------------------------------------------------------ 4. ffmpeg
log "4/6 ffmpeg $FFMPEG_VERSION (statyczny .exe: NVENC/NVDEC/CUDA-LLVM, QSV + D3D11, dav1d, $TARGET_CPU)"
FFPREFIX="$CACHE/ffmpeg-$FFMPEG_VERSION"
FFSTAMP="$DEPSTAMP cpu=$TARGET_CPU"
if [[ "$FORCE" = 1 || ! -f "$FFPREFIX/bin/ffmpeg.exe" || "$(cat "$FFPREFIX/.stamp" 2>/dev/null)" != "$FFSTAMP" ]]; then
    SRC="$WORK/ffmpeg-$FFMPEG_VERSION"
    # Archiwum źródeł wspólne z buildem Linux (build/work).
    ffmpeg_unpack "$FFMPEG_VERSION" "$WORK" "$BUILD/work" \
        || fail "nie udało się pobrać źródeł ffmpeg $FFMPEG_VERSION (ffmpeg.org ani GitHub)"
    (
        cd "$SRC"
        # Tylko nasze .pc — żadnych bibliotek Linuksa z kontenera.
        export PKG_CONFIG_LIBDIR="$DEPS/lib/pkgconfig:$DEPS/share/pkgconfig"
        export PKG_CONFIG_PATH=""
        ./configure \
            --prefix="$FFPREFIX" \
            --target-os=mingw32 --arch=x86_64 --cross-prefix="$TRIPLE-" \
            --cc="$CC_W" --cxx="$CXX_W" \
            --pkg-config=pkg-config --pkg-config-flags=--static \
            --extra-cflags="-march=$TARGET_CPU -O2 -I$DEPS/include" \
            --extra-ldflags="-static -L$DEPS/lib" \
            --enable-gpl \
            --disable-autodetect --disable-doc --disable-ffplay --disable-debug \
            --disable-shared --enable-static \
            --enable-ffnvcodec --enable-nvenc --enable-nvdec --enable-cuda --enable-cuda-llvm \
            --enable-d3d11va --enable-dxva2 --enable-libvpl \
            --enable-libdav1d --enable-zlib
        make -j"$JOBS"
        make install
        # ffmpeg.exe nie uruchomi się w kontenerze — sprawdzamy, co trafiło do konfiguracji.
        # Od FFmpeg 5.1 lista komponentów (CONFIG_*_ENCODER itd.) jest w config_components.h.
        cp config.h "$FFPREFIX/config.h"
        [[ -f config_components.h ]] && cp config_components.h "$FFPREFIX/config_components.h"
    )
    echo "$FFSTAMP" > "$FFPREFIX/.stamp"
fi
# ffmpeg zbudowany wcześniej (cache) mógł nie mieć skopiowanego config_components.h — weź go ze źródeł.
SRC="$WORK/ffmpeg-$FFMPEG_VERSION"
if [[ ! -f "$FFPREFIX/config_components.h" && -f "$SRC/config_components.h" ]]; then
    cp "$SRC/config_components.h" "$FFPREFIX/config_components.h"
fi
FFCONF="$(cat "$FFPREFIX/config.h" "$FFPREFIX/config_components.h" 2>/dev/null || true)"
for c in HEVC_NVENC_ENCODER AV1_NVENC_ENCODER HEVC_QSV_ENCODER AV1_QSV_ENCODER EAC3_ENCODER \
         H264_QSV_DECODER HEVC_QSV_DECODER AV1_QSV_DECODER LIBDAV1D_DECODER \
         HWUPLOAD_CUDA_FILTER HWDOWNLOAD_FILTER BILATERAL_CUDA_FILTER HQDN3D_FILTER CROPDETECT_FILTER \
         VPP_QSV_FILTER HWUPLOAD_FILTER MATROSKA_MUXER MP4_MUXER; do
    grep -q "^#define CONFIG_$c 1" <<<"$FFCONF" || fail "ffmpeg (Windows) bez $c"
done
echo "ffmpeg.exe: NVENC, QSV, CUDA i filtry obecne"

# ------------------------------------------------------------------ 5. MKVToolNix (mkvmerge.exe)
log "5/6 MKVToolNix $MKVTOOLNIX_VERSION (wersja przenośna, mkvmerge.exe)"
MKV="$CACHE/mkvtoolnix-$MKVTOOLNIX_VERSION"
if [[ ! -f "$MKV/mkvtoolnix/mkvmerge.exe" ]]; then
    ARCHIVE="$WORK/mkvtoolnix-64-bit-$MKVTOOLNIX_VERSION.7z"
    wget -nv -O "$ARCHIVE" \
        "https://mkvtoolnix.download/windows/releases/$MKVTOOLNIX_VERSION/mkvtoolnix-64-bit-$MKVTOOLNIX_VERSION.7z" \
        || fail "nie udało się pobrać MKVToolNix $MKVTOOLNIX_VERSION"
    rm -rf "$MKV" && mkdir -p "$MKV"
    "$SEVENZ" x -y -o"$MKV" "$ARCHIVE" >/dev/null
    [[ -f "$MKV/mkvtoolnix/mkvmerge.exe" ]] || fail "w archiwum MKVToolNix brak mkvtoolnix/mkvmerge.exe"
fi

# MediaInfo CLI (oficjalna paczka MediaArea dla Windows x64).
MI="$CACHE/mediainfo-$MEDIAINFO_VERSION"
if ! compgen -G "$MI/*.exe" >/dev/null; then
    ARCHIVE="$WORK/MediaInfo_CLI_${MEDIAINFO_VERSION}_Windows_x64.zip"
    wget -nv -O "$ARCHIVE" \
        "https://mediaarea.net/download/binary/mediainfo/$MEDIAINFO_VERSION/MediaInfo_CLI_${MEDIAINFO_VERSION}_Windows_x64.zip" \
        || fail "nie udało się pobrać MediaInfo CLI $MEDIAINFO_VERSION"
    rm -rf "$MI" && mkdir -p "$MI"
    "$SEVENZ" x -y -o"$MI" "$ARCHIVE" >/dev/null
    compgen -G "$MI/*.exe" >/dev/null || fail "w archiwum MediaInfo CLI brak pliku .exe"
fi

# mpv (odtwarzacz pod F3). Kompilacja skrośna mpv z zależnościami (libplacebo, libass, ANGLE/Vulkan…)
# jest bardzo ciężka, więc — jak MKVToolNix — bierzemy gotowy, statycznie zlinkowany build
# shinchiro/mpv-winbuild-cmake w wariancie x86-64-v3 (Linux: mpv kompilowany ze źródeł).
MPVW="$CACHE/mpv-win-$MPV_WIN_TAG"
if [[ ! -f "$MPVW/mpv.exe" ]]; then
    REL="$WORK/mpv-release.json"
    wget -qO "$REL" "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/tags/$MPV_WIN_TAG" \
        || fail "nie udało się pobrać listy plików mpv $MPV_WIN_TAG z GitHuba"
    URL="$({ grep -oE '"browser_download_url": *"[^"]+/mpv-x86_64-v3-[^"/]+\.7z"' "$REL" || true; } \
        | grep -v -- '-dev-' | head -n1 | sed -E 's/.*"([^"]+)"$/\1/')"
    [[ -n "$URL" ]] || fail "w wydaniu mpv $MPV_WIN_TAG brak archiwum mpv-x86_64-v3-*.7z"
    ARCHIVE="$WORK/$(basename "$URL")"
    wget -nv -O "$ARCHIVE" "$URL" || fail "nie udało się pobrać $URL"
    rm -rf "$MPVW" && mkdir -p "$MPVW"
    "$SEVENZ" x -y -o"$MPVW" "$ARCHIVE" >/dev/null
    [[ -f "$MPVW/mpv.exe" ]] || fail "w archiwum mpv brak mpv.exe"
fi

# Terminal (Alacritty) — okno z TUI z ikoną VideoPipeline na pasku zadań i w ramce okna.
# build.rs Alacritty dołącza zasoby (ikona, manifest) tylko przy budowaniu na Windows (#[cfg(windows)]
# w skrypcie budowania), więc przy kroskompilacji robimy to sami: windres + -C link-arg.
# Gdy budowanie się nie uda, paczka powstaje bez terminala (program użyje konsoli Windows).
ICON_TAG="$(sha256sum icons/videopipeline.ico | cut -c1-8)"
TERMW="$CACHE/alacritty-$ALACRITTY_VERSION-$ICON_TAG"
if [[ "$FORCE" = 1 || ! -f "$TERMW/videopipeline-term.exe" ]]; then
    log "terminal: Alacritty $ALACRITTY_VERSION jako videopipeline-term.exe (ze źródeł)"
    build_alacritty_win() {
        rm -rf "$WORK/alacritty" "$TERMW"
        git -c advice.detachedHead=false clone --quiet --depth 1 --branch "v$ALACRITTY_VERSION" \
            https://github.com/alacritty/alacritty.git "$WORK/alacritty" || return 1
        local res="$WORK/alacritty/alacritty/windows"
        [[ -f "$res/alacritty.rc" && -f "$res/alacritty.ico" ]] || { echo "brak $res/alacritty.rc/.ico" >&2; return 1; }
        cp icons/videopipeline.ico "$res/alacritty.ico"
        (cd "$res" && "$TRIPLE-windres" -O coff -i alacritty.rc -o "$WORK/alacritty-res.o") || return 1
        (cd "$WORK/alacritty" && CARGO_TARGET_DIR="$WORK/alacritty/target" \
            RUSTFLAGS="-C target-cpu=$TARGET_CPU -C link-arg=$WORK/alacritty-res.o" \
            cargo build --release --locked -p alacritty --target "$RTARGET") || return 1
        mkdir -p "$TERMW"
        cp "$WORK/alacritty/target/$RTARGET/release/alacritty.exe" "$TERMW/videopipeline-term.exe"
        cp "$WORK/alacritty/LICENSE-APACHE" "$TERMW/LICENSE-ALACRITTY.txt"
        "$TRIPLE-strip" "$TERMW/videopipeline-term.exe" || true
        "$TRIPLE-objdump" -h "$TERMW/videopipeline-term.exe" | grep '\.rsrc' >/dev/null \
            || { echo "videopipeline-term.exe bez zasobów (ikony)" >&2; rm -rf "$TERMW"; return 1; }
    }
    if ! build_alacritty_win; then
        echo "UWAGA: nie udało się zbudować Alacritty dla Windows — paczka bez videopipeline-term.exe" >&2
    fi
fi

# ------------------------------------------------------------------ 6. pakiet .zip
log "6/6 składanie $PKGNAME.zip"
rm -rf "$PKG" && mkdir -p "$PKG"
cp "$OUT/videopipeline.exe" "$OUT/videopipeline_core.dll" "$STD_DLL" "$PKG/"
cp "$FFPREFIX/bin/ffmpeg.exe" "$FFPREFIX/bin/ffprobe.exe" "$PKG/"
cp "$TOOLS/bin/dovi_tool.exe" "$TOOLS/bin/hdr10plus_tool.exe" "$PKG/"
cp "$MKV/mkvtoolnix/mkvmerge.exe" "$PKG/"
# MediaInfo.exe (+ jego biblioteki DLL, jeśli są w paczce CLI); Windows nie rozróżnia wielkości liter,
# więc program znajduje go jako "mediainfo".
cp "$MI"/*.exe "$PKG/"
for d in "$MI"/*.dll "$MI"/*.DLL; do
    if [[ -f "$d" ]]; then cp "$d" "$PKG/"; fi
done
# Terminal z ikoną VideoPipeline (jeśli zbudowany) i jego konfiguracja.
if [[ -f "$TERMW/videopipeline-term.exe" ]]; then
    cp "$TERMW/videopipeline-term.exe" "$TERMW/LICENSE-ALACRITTY.txt" "$PKG/"
    cp packaging/videopipeline-term.toml "$PKG/"
fi
# mpv.exe (+ ewentualne DLL z paczki); program szuka mpv najpierw obok videopipeline.exe.
cp "$MPVW/mpv.exe" "$PKG/"
for d in "$MPVW"/*.dll; do
    if [[ -f "$d" ]]; then cp "$d" "$PKG/"; fi
done
# Dane mkvmerge (baza typów plików itp.), jeśli są w wersji przenośnej.
[[ -d "$MKV/mkvtoolnix/data" ]] && cp -r "$MKV/mkvtoolnix/data" "$PKG/"
"$TRIPLE-strip" "$PKG/ffmpeg.exe" "$PKG/ffprobe.exe" "$PKG/dovi_tool.exe" "$PKG/hdr10plus_tool.exe" 2>/dev/null || true

# Ikona w zasobach videopipeline.exe (sekcja .rsrc z app/build.rs).
"$TRIPLE-objdump" -h "$PKG/videopipeline.exe" | grep '\.rsrc' >/dev/null || fail "videopipeline.exe bez zasobów (ikony) — sprawdź windres"

# Zależności DLL: wszystko poza bibliotekami systemu Windows musi leżeć w pakiecie.
SYSTEM_DLL='^(kernel32|kernelbase|user32|advapi32|shell32|shlwapi|ole32|oleaut32|gdi32|ws2_32|bcrypt|bcryptprimitives|secur32|crypt32|ncrypt|ntdll|msvcrt|ucrtbase|api-ms-win-.*|ext-ms-win-.*|comdlg32|comctl32|winmm|psapi|setupapi|cfgmgr32|dxgi|d3d9|d3d11|d3d12|dxva2|mfplat|mf|mfreadwrite|evr|version|userenv|iphlpapi|dbghelp|imm32|pdh|powrprof|normaliz|wldap32|rpcrt4|winhttp|wininet|uxtheme|dwmapi|opengl32|mpr|netapi32|avrt|mscms|propsys|synchronization|winspool\.drv|avicap32|msvfw32|msacm32|ksuser|shcore|dwrite|d2d1|d3dcompiler_47|mfcore|mfuuid|cryptbase|wintrust|hid|xinput1_4|dsound|dinput8|vulkan-1|d3dcompiler_43)\.dll$'
MINGW_DIRS=(/usr/lib/gcc/$TRIPLE/*-posix "/usr/$TRIPLE/lib")
changed=1
while [[ $changed = 1 ]]; do
    changed=0
    for f in "$PKG"/*.exe "$PKG"/*.dll; do
        while read -r dll; do
            lc="${dll,,}"
            [[ "$lc" =~ $SYSTEM_DLL ]] && continue
            # Biblioteka systemowa Windows, jeśli MinGW-w64 ma dla niej bibliotekę importu (np. libauthz.a).
            [[ -f "/usr/$TRIPLE/lib/lib${lc%.dll}.a" ]] && continue
            [[ -n "$(find "$PKG" -maxdepth 1 -iname "$dll" -print -quit)" ]] && continue
            src="$(find "${MINGW_DIRS[@]}" -maxdepth 1 -iname "$dll" -print -quit 2>/dev/null || true)"
            [[ -n "$src" ]] || fail "$(basename "$f") wymaga $dll, którego nie ma w pakiecie ani w MinGW"
            echo "dołączam $dll (dla $(basename "$f"))"
            cp "$src" "$PKG/"
            changed=1
        done < <("$TRIPLE-objdump" -p "$f" | sed -n 's/^\s*DLL Name: //p')
    done
done

cp LICENSE "$PKG/LICENSE.txt"
cp packaging/appimage/THIRD-PARTY-NOTICES.md "$PKG/"
cp packaging/windows/README-Windows.txt "$PKG/"
# Skrót „VideoPipeline” (pulpit + menu Start) uruchamiający program w klasycznej konsoli (conhost),
# w której pasek zadań pokazuje ikonę programu. Pliki .cmd/.ps1 z końcami linii CRLF.
for f in utworz-skrot.cmd utworz-skrot.ps1; do sed 's/$/\r/' "packaging/windows/$f" > "$PKG/$f"; done
cp icons/videopipeline-256.png "$PKG/videopipeline.png"

ZIP="$DIST/$PKGNAME.zip"
rm -f "$ZIP"
(cd "$WORK" && zip -qr9 "$ZIP" "$PKGNAME")
(cd "$DIST" && sha256sum "$(basename "$ZIP")" > "$(basename "$ZIP").sha256")
ls -1 "$PKG"
log "Gotowe: $ZIP ($(du -h "$ZIP" | cut -f1))"
