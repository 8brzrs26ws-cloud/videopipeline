#!/usr/bin/env bash
# Kroskompilacja wersji Windows x64 (x86_64-pc-windows-gnu, MinGW-w64) w kontenerze Debian 13:
#   dist/VideoPipeline-<wersja>-windows-x86_64_v3.zip (+ .sha256)
# Zawartość: videopipeline.exe + videopipeline_core.dll + std-*.dll, ffmpeg.exe i ffprobe.exe
# (statyczne: NVENC/NVDEC/CUDA-LLVM, Intel QSV przez oneVPL + D3D11, dav1d, zlib), mkvmerge.exe
# (oficjalna wersja przenośna MKVToolNix), dovi_tool.exe, hdr10plus_tool.exe.
# Sterowniki GPU (NVIDIA, Intel) pochodzą z systemu Windows.
#
# Zmienne: VERSION, FFMPEG_VERSION (latest = najnowsze wydanie), DOVI_TOOL_REF, HDR10PLUS_TOOL_REF,
# DAV1D_REF, LIBVPL_REF, MKVTOOLNIX_VERSION (latest = najnowsze), NVCODEC_HEADERS_REF, TARGET_CPU, JOBS, FORCE=1.
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

for c in cargo rustc "$CC_W" "$CXX_W" "$TRIPLE-objdump" "$TRIPLE-strip" meson ninja cmake nasm clang \
         pkg-config git wget zip; do need "$c"; done
SEVENZ="$(command -v 7zz || command -v 7z || true)"
[[ -n "$SEVENZ" ]] || fail "brak 7zz/7z (pakiet 7zip)"
rustup target list --installed 2>/dev/null | grep -qx "$RTARGET" \
    || fail "brak celu Rust $RTARGET (rustup target add $RTARGET — robi to docker/Dockerfile)"

# ------------------------------------------------------------------ wersje "latest"
# Każde ustalenie wersji jest odporne na pipefail i kończy się czytelnym błędem (wcześniej skrypt
# potrafił zakończyć się bez komunikatu, gdy np. strona z listą wersji nie odpowiedziała).
latest_git_tag() {   # $1 = repo, $2 = opcjonalny prefiks tagu (np. "release-"); najwyższy tag semver
    local pre="${2:-}"
    { git ls-remote --tags --refs "$1" 2>/dev/null || true; } | sed 's|.*refs/tags/||' \
        | { grep -E "^${pre}v?[0-9]+(\.[0-9]+)+\$" || true; } | sed -E "s/^${pre}v?//" | sort -V | tail -n1
}
resolve() {   # $1 = nazwa zmiennej, $2 = opis źródła, reszta = polecenie wypisujące wersję
    local name="$1" what="$2"; shift 2
    [[ "${!name}" = latest ]] || return 0
    local v; v="$("$@" || true)"
    [[ -n "$v" ]] || fail "nie udało się ustalić najnowszej wersji: $name ($what) — podaj ją ręcznie, np. $name=..."
    printf -v "$name" '%s' "$v"
}
ffmpeg_latest() {
    { wget -qO- https://ffmpeg.org/releases/ || true; } | { grep -oE 'ffmpeg-[0-9]+\.[0-9]+(\.[0-9]+)?\.tar\.xz' || true; } \
        | sed -E 's/ffmpeg-(.*)\.tar\.xz/\1/' | sort -uV | tail -n1
}
resolve FFMPEG_VERSION "ffmpeg.org/releases" ffmpeg_latest
resolve DOVI_TOOL_REF "github.com/quietvoid/dovi_tool" latest_git_tag https://github.com/quietvoid/dovi_tool
resolve HDR10PLUS_TOOL_REF "github.com/quietvoid/hdr10plus_tool" latest_git_tag https://github.com/quietvoid/hdr10plus_tool
resolve DAV1D_REF "code.videolan.org/videolan/dav1d" latest_git_tag https://code.videolan.org/videolan/dav1d.git
# libvpl: tylko wydania 2.x — repozytorium ma też stare tagi oneVPL "v2023.x", które sortują się wyżej.
libvpl_latest() {
    { git ls-remote --tags --refs https://github.com/intel/libvpl.git 2>/dev/null || true; } | sed 's|.*refs/tags/||' \
        | { grep -E '^v2\.[0-9]+\.[0-9]+$' || true; } | sed 's/^v//' | sort -V | tail -n1
}
resolve LIBVPL_REF "github.com/intel/libvpl" libvpl_latest
# MKVToolNix: tagi "release-X.Y" w repozytorium projektu (Codeberg; zapasowo GitLab).
mkvtoolnix_latest() {
    local v; v="$(latest_git_tag https://codeberg.org/mbunkus/mkvtoolnix.git release-)"
    [[ -n "$v" ]] || v="$(latest_git_tag https://gitlab.com/mbunkus/mkvtoolnix.git release-)"
    echo "$v"
}
resolve MKVTOOLNIX_VERSION "tagi release-* MKVToolNix" mkvtoolnix_latest
echo "Wersje: ffmpeg $FFMPEG_VERSION, dav1d $DAV1D_REF, libvpl $LIBVPL_REF, MKVToolNix $MKVTOOLNIX_VERSION," \
     "dovi_tool $DOVI_TOOL_REF, hdr10plus_tool $HDR10PLUS_TOOL_REF"

export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$TRIPLE-gcc"

# ------------------------------------------------------------------ 1. aplikacja
log "1/6 videopipeline.exe + videopipeline_core.dll ($TARGET_CPU)"
# RUSTFLAGS zastępuje .cargo/config.toml — prefer-dynamic podajemy jawnie (biblioteka core jako DLL).
RUSTFLAGS="-C prefer-dynamic -C target-cpu=$TARGET_CPU" cargo build --release --target "$RTARGET"
OUT="target/$RTARGET/release"
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
    TARBALL="$BUILD/work/ffmpeg-$FFMPEG_VERSION.tar.xz"   # wspólny z buildem Linux
    [[ -f "$TARBALL" ]] || wget -q -O "$TARBALL" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    rm -rf "$SRC" && tar -C "$WORK" -xf "$TARBALL"
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

# ------------------------------------------------------------------ 6. pakiet .zip
log "6/6 składanie $PKGNAME.zip"
rm -rf "$PKG" && mkdir -p "$PKG"
cp "$OUT/videopipeline.exe" "$OUT/videopipeline_core.dll" "$STD_DLL" "$PKG/"
cp "$FFPREFIX/bin/ffmpeg.exe" "$FFPREFIX/bin/ffprobe.exe" "$PKG/"
cp "$TOOLS/bin/dovi_tool.exe" "$TOOLS/bin/hdr10plus_tool.exe" "$PKG/"
cp "$MKV/mkvtoolnix/mkvmerge.exe" "$PKG/"
# Dane mkvmerge (baza typów plików itp.), jeśli są w wersji przenośnej.
[[ -d "$MKV/mkvtoolnix/data" ]] && cp -r "$MKV/mkvtoolnix/data" "$PKG/"
"$TRIPLE-strip" "$PKG/ffmpeg.exe" "$PKG/ffprobe.exe" "$PKG/dovi_tool.exe" "$PKG/hdr10plus_tool.exe" 2>/dev/null || true

# Zależności DLL: wszystko poza bibliotekami systemu Windows musi leżeć w pakiecie.
SYSTEM_DLL='^(kernel32|kernelbase|user32|advapi32|shell32|shlwapi|ole32|oleaut32|gdi32|ws2_32|bcrypt|bcryptprimitives|secur32|crypt32|ncrypt|ntdll|msvcrt|ucrtbase|api-ms-win-.*|ext-ms-win-.*|comdlg32|comctl32|winmm|psapi|setupapi|cfgmgr32|dxgi|d3d9|d3d11|d3d12|dxva2|mfplat|mf|mfreadwrite|evr|version|userenv|iphlpapi|dbghelp|imm32|pdh|powrprof|normaliz|wldap32|rpcrt4|winhttp|wininet|uxtheme|dwmapi|opengl32|mpr|netapi32|avrt|mscms|propsys|synchronization|winspool\.drv|avicap32|msvfw32|msacm32|ksuser|shcore|dwrite|d2d1|d3dcompiler_47|mfcore|mfuuid|cryptbase|wintrust|hid|xinput1_4|dsound|dinput8)\.dll$'
MINGW_DIRS=(/usr/lib/gcc/$TRIPLE/*-posix "/usr/$TRIPLE/lib")
changed=1
while [[ $changed = 1 ]]; do
    changed=0
    for f in "$PKG"/*.exe "$PKG"/*.dll; do
        while read -r dll; do
            lc="${dll,,}"
            [[ "$lc" =~ $SYSTEM_DLL ]] && continue
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
cp icons/videopipeline-256.png "$PKG/videopipeline.png"

ZIP="$DIST/$PKGNAME.zip"
rm -f "$ZIP"
(cd "$WORK" && zip -qr9 "$ZIP" "$PKGNAME")
(cd "$DIST" && sha256sum "$(basename "$ZIP")" > "$(basename "$ZIP").sha256")
ls -1 "$PKG"
log "Gotowe: $ZIP ($(du -h "$ZIP" | cut -f1))"
