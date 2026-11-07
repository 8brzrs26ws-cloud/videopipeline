#!/usr/bin/env bash
# Buduje dist/VideoPipeline-<wersja>-x86_64_v3.AppImage z dołączonymi narzędziami:
#   ffmpeg + ffprobe (NVENC/NVDEC/CUDA-LLVM, statycznie), mkvmerge, dovi_tool, hdr10plus_tool.
# Kod własny (videopipeline, biblioteka core, dovi_tool, hdr10plus_tool, ffmpeg) jest kompilowany
# z -march/-C target-cpu=x86-64-v3. Sterowniki NVIDIA pochodzą z hosta i NIE są dołączane.
#
# Wymagania (Ubuntu/Debian):
#   build-essential nasm pkg-config clang patchelf file wget git mkvtoolnix libdav1d-dev zlib1g-dev
#   + rustup/cargo. Uruchamiane w CI przez .github/workflows/release.yml, ale działa też lokalnie.
#
# Zmienne: VERSION, FFMPEG_VERSION, NVCODEC_HEADERS_REF (domyślnie master), DOVI_TOOL_REF, HDR10PLUS_TOOL_REF
# (dla trzech pierwszych narzędzi "latest" = najnowsze wydanie), INTEL=auto|1|0 (VAAPI + QSV/libvpl w ffmpeg),
# TARGET_CPU, JOBS, FORCE=1. Zalecane uruchamianie: ./build-local.sh (Docker, Debian 13).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="${VERSION:-$(grep -m1 '^version' app/Cargo.toml | cut -d'"' -f2)}"
FFMPEG_VERSION="${FFMPEG_VERSION:-7.1.1}"
NVCODEC_HEADERS_REF="${NVCODEC_HEADERS_REF:-}"
# dovi_tool i hdr10plus_tool nie są na crates.io (tylko biblioteki dolby_vision/hdr10plus) — budujemy z GitHuba, z tagów.
DOVI_TOOL_REF="${DOVI_TOOL_REF:-2.3.4}"
HDR10PLUS_TOOL_REF="${HDR10PLUS_TOOL_REF:-1.7.2}"
TARGET_CPU="${TARGET_CPU:-x86-64-v3}"
INTEL="${INTEL:-auto}"
JOBS="${JOBS:-$(nproc)}"
FORCE="${FORCE:-0}"

BUILD="$ROOT/build"
CACHE="$BUILD/cache"     # trwałe artefakty (cache w CI): ffmpeg, dovi_tool, hdr10plus_tool, linuxdeploy
WORK="$BUILD/work"       # katalog roboczy (AppDir)
APPDIR="$WORK/AppDir"
DIST="$ROOT/dist"
mkdir -p "$CACHE" "$WORK" "$DIST"

log()  { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mBŁĄD: %s\033[0m\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "brak polecenia: $1"; }

# "latest" → najnowsze stabilne wydanie.
latest_git_tag() {   # $1 = repo URL; zwraca najwyższy tag semver (bez prefiksu v)
    git ls-remote --tags --refs "$1" | sed 's|.*refs/tags/||' | grep -E '^v?[0-9]+(\.[0-9]+)+$' \
        | sed 's/^v//' | sort -V | tail -n1
}
if [[ "$FFMPEG_VERSION" = latest ]]; then
    FFMPEG_VERSION="$(wget -qO- https://ffmpeg.org/releases/ | grep -oE 'ffmpeg-[0-9]+\.[0-9]+(\.[0-9]+)?\.tar\.xz' \
        | sed -E 's/ffmpeg-(.*)\.tar\.xz/\1/' | sort -uV | tail -n1)"
    [[ -n "$FFMPEG_VERSION" ]] || fail "nie udało się ustalić najnowszej wersji ffmpeg"
fi
[[ "$DOVI_TOOL_REF" = latest ]] && DOVI_TOOL_REF="$(latest_git_tag https://github.com/quietvoid/dovi_tool)"
[[ "$HDR10PLUS_TOOL_REF" = latest ]] && HDR10PLUS_TOOL_REF="$(latest_git_tag https://github.com/quietvoid/hdr10plus_tool)"
echo "Wersje: ffmpeg $FFMPEG_VERSION, dovi_tool $DOVI_TOOL_REF, hdr10plus_tool $HDR10PLUS_TOOL_REF"

for c in cargo rustc gcc clang make nasm pkg-config git wget patchelf mkvmerge; do need "$c"; done
pkg-config --exists dav1d || fail "brak libdav1d-dev (pkg-config dav1d)"

# --------------------------------------------------------------------------- 1. aplikacja
log "1/5 videopipeline + biblioteka core ($TARGET_CPU)"
VP_RUSTFLAGS_EXTRA="-C target-cpu=$TARGET_CPU" bash ./build.sh
[[ -x target/release/videopipeline && -f target/release/libvideopipeline_core.so ]] \
    || fail "build.sh nie wytworzył binarki/biblioteki"

# --------------------------------------------------------------------------- 2. dovi_tool, hdr10plus_tool
log "2/5 dovi_tool $DOVI_TOOL_REF (wbudowana czcionka, bez fontconfig) + hdr10plus_tool $HDR10PLUS_TOOL_REF (cargo install --git, $TARGET_CPU)"
TOOLS="$CACHE/tools"
STAMP="dovi_tool=$DOVI_TOOL_REF+internal-font hdr10plus_tool=$HDR10PLUS_TOOL_REF+internal-font cpu=$TARGET_CPU"
if [[ "$FORCE" = 1 || ! -x "$TOOLS/bin/dovi_tool" || ! -x "$TOOLS/bin/hdr10plus_tool" \
      || "$(cat "$TOOLS/.stamp" 2>/dev/null)" != "$STAMP" ]]; then
    export RUSTFLAGS="-C target-cpu=$TARGET_CPU"
    export CARGO_TARGET_DIR="$WORK/tools-target"
    cargo install --locked --force --root "$TOOLS" \
        --git https://github.com/quietvoid/dovi_tool --tag "$DOVI_TOOL_REF" \
        --no-default-features --features internal-font dovi_tool
    cargo install --locked --force --root "$TOOLS" \
        --git https://github.com/quietvoid/hdr10plus_tool --tag "$HDR10PLUS_TOOL_REF" \
        --no-default-features --features internal-font hdr10plus_tool
    unset RUSTFLAGS CARGO_TARGET_DIR
    echo "$STAMP" > "$TOOLS/.stamp"
fi
"$TOOLS/bin/dovi_tool" --version
"$TOOLS/bin/hdr10plus_tool" --version

# --------------------------------------------------------------------------- 3. ffmpeg
log "3/5 ffmpeg $FFMPEG_VERSION (statyczny, NVENC/NVDEC/CUDA-LLVM + Intel VAAPI/QSV jeśli dostępne, $TARGET_CPU)"
# Intel (VAAPI + QSV przez libvpl): wymaga statycznych libva/libvpl; sterownik VA (iHD) zostaje po stronie hosta.
intel_available() {
    pkg-config --exists libva libva-drm vpl 2>/dev/null || return 1
    [[ -f "$(pkg-config --variable=libdir libva)/libva.a" && -f "$(pkg-config --variable=libdir vpl)/libvpl.a" ]]
}
USE_INTEL=0
case "$INTEL" in
    0) ;;
    1) intel_available || fail "INTEL=1, ale brak statycznych libva/libvpl (libva-dev, libvpl-dev)"; USE_INTEL=1 ;;
    *) if intel_available; then USE_INTEL=1; else echo "UWAGA: brak statycznych libva/libvpl — ffmpeg bez Intela"; fi ;;
esac

NVH="$CACHE/nv-codec-headers"
prefix_for() { echo "$CACHE/ffmpeg-$FFMPEG_VERSION-gpl$([[ $1 = 1 ]] && echo -intel)"; }

build_ffmpeg() {   # $1 = 1|0 (Intel); prefiks = prefix_for $1
    local intel="$1" prefix; prefix="$(prefix_for "$1")"
    local extra=()
    [[ "$intel" = 1 ]] && extra=(--enable-vaapi --enable-libvpl)
    rm -rf "$NVH" "$WORK/nv-codec-headers"
    git clone --quiet https://github.com/FFmpeg/nv-codec-headers.git "$WORK/nv-codec-headers"
    [[ -n "$NVCODEC_HEADERS_REF" ]] && git -C "$WORK/nv-codec-headers" checkout --quiet "$NVCODEC_HEADERS_REF"
    make -C "$WORK/nv-codec-headers" PREFIX="$NVH" install >/dev/null

    local src="$WORK/ffmpeg-$FFMPEG_VERSION"
    rm -rf "$src"
    [[ -f "$WORK/ffmpeg-$FFMPEG_VERSION.tar.xz" ]] \
        || wget -q -O "$WORK/ffmpeg-$FFMPEG_VERSION.tar.xz" "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz"
    tar -C "$WORK" -xf "$WORK/ffmpeg-$FFMPEG_VERSION.tar.xz"
    (
        cd "$src"
        export PKG_CONFIG_PATH="$NVH/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
        ./configure \
            --prefix="$prefix" \
            --pkg-config-flags=--static \
            --extra-cflags="-march=$TARGET_CPU -O2" \
            --enable-gpl \
            --disable-autodetect --disable-doc --disable-ffplay --disable-debug \
            --disable-shared --enable-static \
            --enable-ffnvcodec --enable-nvenc --enable-nvdec --enable-cuda --enable-cuda-llvm \
            --enable-libdav1d --enable-zlib "${extra[@]}"
        make -j"$JOBS"
        make install
    )
}

# Katalog z sufiksem (-gpl, -intel): stary cache o innej konfiguracji nie zostanie użyty ponownie.
FFPREFIX="$(prefix_for "$USE_INTEL")"
if [[ "$FORCE" = 1 || ! -x "$FFPREFIX/bin/ffmpeg" ]]; then
    if ! build_ffmpeg "$USE_INTEL"; then
        if [[ "$USE_INTEL" = 1 && "$INTEL" = auto ]]; then
            echo "UWAGA: ffmpeg z Intelem się nie zbudował — próba bez Intela (INTEL=1 wymusza błąd)"
            USE_INTEL=0
            FFPREFIX="$(prefix_for 0)"
            build_ffmpeg 0
        else
            fail "budowanie ffmpeg nie powiodło się"
        fi
    fi
fi

FFMPEG="$FFPREFIX/bin/ffmpeg"
[[ -x "$FFMPEG" ]] || fail "brak $FFMPEG"
# Sprawdzamy, że ffmpeg ma wszystko, czego wymaga videopipeline (lista nie wymaga GPU).
ENCODERS="$("$FFMPEG" -hide_banner -encoders 2>/dev/null)"
FILTERS="$("$FFMPEG" -hide_banner -filters 2>/dev/null)"
for e in hevc_nvenc av1_nvenc eac3; do
    grep -qw "$e" <<<"$ENCODERS" || fail "ffmpeg bez enkodera $e"
done
for f in hwupload_cuda hwdownload bilateral_cuda hqdn3d cropdetect; do
    grep -qw "$f" <<<"$FILTERS" || fail "ffmpeg bez filtra $f"
done
if [[ "$USE_INTEL" = 1 || "$FFPREFIX" == *-intel ]]; then
    for e in hevc_qsv av1_qsv hevc_vaapi; do
        grep -qw "$e" <<<"$ENCODERS" || fail "ffmpeg (Intel) bez enkodera $e"
    done
    echo "ffmpeg: Intel VAAPI + QSV włączone"
fi
"$FFMPEG" -hide_banner -version | sed -n 1p

# --------------------------------------------------------------------------- 4. AppDir
log "4/5 składanie AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/licenses/videopipeline" "$APPDIR/usr/share/misc"

install -m755 target/release/videopipeline "$APPDIR/usr/bin/"
install -m755 target/release/libvideopipeline_core.so "$APPDIR/usr/lib/"
# Rust dylib: binarka i biblioteka core potrzebują libstd z sysroot rustc.
SYSROOT="$(rustc --print sysroot)"
STD_SO="$(find "$SYSROOT/lib" -name 'libstd-*.so' | head -n1)"
[ -n "$STD_SO" ] || { echo "BŁĄD: brak libstd-*.so w $SYSROOT/lib"; exit 1; }
echo "libstd: $STD_SO"
cp "$STD_SO" "$APPDIR/usr/lib/"

install -m755 "$FFPREFIX/bin/ffmpeg" "$FFPREFIX/bin/ffprobe" "$APPDIR/usr/bin/"
install -m755 "$TOOLS/bin/dovi_tool" "$TOOLS/bin/hdr10plus_tool" "$APPDIR/usr/bin/"
install -m755 "$(command -v mkvmerge)" "$APPDIR/usr/bin/"
for m in /usr/lib/file/magic.mgc /usr/share/misc/magic.mgc; do
    if [[ -f "$m" ]]; then cp -L "$m" "$APPDIR/usr/share/misc/magic.mgc"; break; fi
done

install -m644 packaging/appimage/videopipeline.desktop "$APPDIR/usr/share/applications/"
install -m644 LICENSE "$APPDIR/usr/share/licenses/videopipeline/LICENSE"
install -m644 packaging/appimage/THIRD-PARTY-NOTICES.md "$APPDIR/usr/share/licenses/videopipeline/"

# Kontrola: aplikacja ma znaleźć libstd i bibliotekę core w AppDir (rpath $ORIGIN/../lib).
ldd "$APPDIR/usr/bin/videopipeline" > "$WORK/ldd.txt" || true
if grep -q 'not found' "$WORK/ldd.txt"; then
    cat "$WORK/ldd.txt" >&2
    fail "binarka videopipeline nie znajduje wszystkich bibliotek w AppDir"
fi

# --------------------------------------------------------------------------- 5. obraz
log "5/5 linuxdeploy → AppImage"
BIN="$CACHE/bin"
mkdir -p "$BIN"
dl() {   # $1 = repo linuxdeploy/<repo>, $2 = nazwa pliku w wydaniu "continuous"
    local f="$BIN/$2"
    if [[ ! -x "$f" ]]; then
        wget -nv -O "$f.part" "https://github.com/linuxdeploy/$1/releases/download/continuous/$2" \
            || fail "nie udało się pobrać $2 z github.com/linuxdeploy/$1"
        mv "$f.part" "$f"; chmod +x "$f"
    fi
}
dl linuxdeploy linuxdeploy-x86_64.AppImage
dl linuxdeploy-plugin-appimage linuxdeploy-plugin-appimage-x86_64.AppImage
# linuxdeploy szuka wtyczki "linuxdeploy-plugin-appimage" w PATH.
ln -sf "$BIN/linuxdeploy-plugin-appimage-x86_64.AppImage" "$BIN/linuxdeploy-plugin-appimage"

export PATH="$BIN:$PATH"
export APPIMAGE_EXTRACT_AND_RUN=1     # runner CI nie ma FUSE
export NO_STRIP=1
export ARCH=x86_64
export LINUXDEPLOY_OUTPUT_VERSION="$VERSION"

(
    cd "$WORK"
    rm -f VideoPipeline-*.AppImage
    "$BIN/linuxdeploy-x86_64.AppImage" \
        --appdir "$APPDIR" \
        --executable "$APPDIR/usr/bin/videopipeline" \
        --executable "$APPDIR/usr/bin/mkvmerge" \
        --executable "$APPDIR/usr/bin/ffmpeg" \
        --executable "$APPDIR/usr/bin/ffprobe" \
        --executable "$APPDIR/usr/bin/dovi_tool" \
        --executable "$APPDIR/usr/bin/hdr10plus_tool" \
        --desktop-file "$APPDIR/usr/share/applications/videopipeline.desktop" \
        --icon-file "$ROOT/icons/videopipeline-256.png" --icon-filename videopipeline \
        --custom-apprun "$ROOT/packaging/appimage/AppRun" \
        --output appimage
)

shopt -s nullglob
images=("$WORK"/VideoPipeline-*.AppImage)
[[ ${#images[@]} -ge 1 ]] || fail "linuxdeploy nie wytworzył pliku AppImage"
OUT="${images[0]}"
FINAL="$DIST/VideoPipeline-$VERSION-x86_64_v3.AppImage"
mv -f "$OUT" "$FINAL"
chmod +x "$FINAL"
(cd "$DIST" && sha256sum "$(basename "$FINAL")" > "$(basename "$FINAL").sha256")

log "Gotowe: $FINAL ($(du -h "$FINAL" | cut -f1))"
