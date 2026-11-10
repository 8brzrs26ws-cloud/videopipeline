#!/usr/bin/env bash
# Buduje dist/VideoPipeline-<wersja>-x86_64_v3.AppImage z dołączonymi narzędziami:
#   ffmpeg + ffprobe (NVENC/NVDEC/CUDA-LLVM statycznie; Intel QSV/VAAPI z libva/libvpl linkowanymi dynamicznie),
#   mkvmerge, mediainfo, dovi_tool, hdr10plus_tool oraz odtwarzacz mpv (F3, budowany ze źródeł; MPV_VERSION)
#   i terminal videopipeline-term (Alacritty ze źródeł z ikoną VideoPipeline; ALACRITTY_VERSION).
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
# Odtwarzacz mpv (F3) — budowany ze źródeł (tag vX.Y.Z z github.com/mpv-player/mpv); "latest" = najnowsze wydanie.
MPV_VERSION="${MPV_VERSION:-latest}"
# Terminal dołączony do paczek: Alacritty (tag vX.Y.Z) z ikoną VideoPipeline, jako videopipeline-term.
ALACRITTY_VERSION="${ALACRITTY_VERSION:-latest}"
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
trap 'fail "przerwano w linii $LINENO: $BASH_COMMAND"' ERR

# "latest" → najnowsze stabilne wydanie.
latest_git_tag() {   # $1 = repo URL; zwraca najwyższy tag semver (bez prefiksu v)
    git ls-remote --tags --refs "$1" | sed 's|.*refs/tags/||' | grep -E '^v?[0-9]+(\.[0-9]+)+$' \
        | sed 's/^v//' | sort -V | tail -n1
}
# shellcheck source=../ffmpeg-source.sh
source "$ROOT/packaging/ffmpeg-source.sh"
if [[ "$FFMPEG_VERSION" = latest ]]; then
    FFMPEG_VERSION="$(ffmpeg_latest "$BUILD/work")"
    [[ -n "$FFMPEG_VERSION" ]] || fail "nie udało się ustalić najnowszej wersji ffmpeg (ffmpeg.org, GitHub, git.ffmpeg.org) — podaj FFMPEG_VERSION=..."
fi
[[ "$DOVI_TOOL_REF" = latest ]] && DOVI_TOOL_REF="$(latest_git_tag https://github.com/quietvoid/dovi_tool)"
[[ "$HDR10PLUS_TOOL_REF" = latest ]] && HDR10PLUS_TOOL_REF="$(latest_git_tag https://github.com/quietvoid/hdr10plus_tool)"
echo "Wersje: ffmpeg $FFMPEG_VERSION, dovi_tool $DOVI_TOOL_REF, hdr10plus_tool $HDR10PLUS_TOOL_REF"

for c in cargo rustc gcc clang make nasm pkg-config git wget patchelf mkvmerge mediainfo meson ninja; do need "$c"; done
pkg-config --exists dav1d || fail "brak libdav1d-dev (pkg-config dav1d)"

# --------------------------------------------------------------------------- 1. aplikacja
log "1/5 videopipeline + biblioteka core ($TARGET_CPU)"
VP_RUSTFLAGS_EXTRA="-C target-cpu=$TARGET_CPU" bash ./build.sh
REL="${CARGO_TARGET_DIR:-target}/release"
[[ -x "$REL/videopipeline" && -f "$REL/libvideopipeline_core.so" ]] \
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
# Intel (VAAPI + QSV przez oneVPL): libva, libdrm i libvpl są linkowane dynamicznie (Debian nie ma ich
# wersji statycznych, a i tak muszą pasować do sterownika iHD i runtime'u oneVPL hosta — patrz AppRun).
intel_available() {
    pkg-config --exists libva libva-drm libdrm vpl 2>/dev/null
}
USE_INTEL=0
case "$INTEL" in
    0) ;;
    1) intel_available || fail "INTEL=1, ale brak libva/libdrm/libvpl (libva-dev, libdrm-dev, libvpl-dev)"; USE_INTEL=1 ;;
    *) if intel_available; then USE_INTEL=1; else echo "UWAGA: brak libva/libdrm/libvpl (-dev) — ffmpeg bez Intela"; fi ;;
esac

NVH="$CACHE/nv-codec-headers"
prefix_for() { echo "$CACHE/ffmpeg-$FFMPEG_VERSION-gpl$([[ $1 = 1 ]] && echo -intel)"; }

build_ffmpeg() {   # $1 = 1|0 (Intel); prefiks = prefix_for $1
    local intel="$1" prefix; prefix="$(prefix_for "$1")"
    local extra=()
    [[ "$intel" = 1 ]] && extra=(--enable-vaapi --enable-libdrm --enable-libvpl)
    rm -rf "$NVH" "$WORK/nv-codec-headers"
    git clone --quiet https://github.com/FFmpeg/nv-codec-headers.git "$WORK/nv-codec-headers"
    [[ -n "$NVCODEC_HEADERS_REF" ]] && git -C "$WORK/nv-codec-headers" checkout --quiet "$NVCODEC_HEADERS_REF"
    make -C "$WORK/nv-codec-headers" PREFIX="$NVH" install >/dev/null

    local src="$WORK/ffmpeg-$FFMPEG_VERSION"
    ffmpeg_unpack "$FFMPEG_VERSION" "$WORK" "$WORK" \
        || fail "nie udało się pobrać źródeł ffmpeg $FFMPEG_VERSION (ffmpeg.org ani GitHub)"
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
    for e in hevc_qsv av1_qsv hevc_vaapi av1_vaapi; do
        grep -qw "$e" <<<"$ENCODERS" || fail "ffmpeg (Intel) bez enkodera $e"
    done
    for f in vpp_qsv scale_vaapi denoise_vaapi hwupload; do
        grep -qw "$f" <<<"$FILTERS" || fail "ffmpeg (Intel) bez filtra $f"
    done
    DECODERS="$("$FFMPEG" -hide_banner -decoders 2>/dev/null)"
    for d in h264_qsv hevc_qsv av1_qsv; do
        grep -qw "$d" <<<"$DECODERS" || fail "ffmpeg (Intel) bez dekodera $d"
    done
    echo "ffmpeg: Intel VAAPI + QSV włączone"
fi
"$FFMPEG" -hide_banner -version | sed -n 1p

# --------------------------------------------------------------------------- 3b. mpv
# Budowany ze źródeł z bibliotekami Debiana (libass, libav*, Lua… z systemu kontenera) — linuxdeploy
# dołączy je do paczki. libplacebo z Debiana bywa za stara dla nowego mpv, więc jest budowana razem
# z mpv jako podprojekt meson (statycznie; LIBPLACEBO_REF, domyślnie najnowszy tag tej samej
# wersji głównej, co wymagana przez mpv). Sterowniki grafiki (GL/EGL/Vulkan), X11/Wayland i ALSA są z hosta.
LIBPLACEBO_REF="${LIBPLACEBO_REF:-latest}"
if [[ "$MPV_VERSION" = latest ]]; then
    MPV_VERSION="$({ GIT_TERMINAL_PROMPT=0 timeout 60 git ls-remote --tags --refs https://github.com/mpv-player/mpv.git 2>/dev/null || true; } \
        | sed 's|.*refs/tags/||' | { grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true; } | sed 's/^v//' | sort -V | tail -n1)"
    if [[ -z "$MPV_VERSION" ]]; then
        MPV_VERSION="$(ls -1d "$CACHE"/mpv-* 2>/dev/null | sed 's|.*/mpv-||' | sort -V | tail -n1)"
    fi
    [[ -n "$MPV_VERSION" ]] || fail "nie udało się ustalić wersji mpv — podaj MPV_VERSION=..."
fi
log "3b/5 mpv $MPV_VERSION (ze źródeł, $TARGET_CPU)"
MPVPREFIX="$CACHE/mpv-$MPV_VERSION"
if [[ "$FORCE" = 1 || ! -x "$MPVPREFIX/bin/mpv" ]]; then
    rm -rf "$WORK/mpv" "$MPVPREFIX"
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "v$MPV_VERSION" \
        https://github.com/mpv-player/mpv.git "$WORK/mpv"
    if [[ "$LIBPLACEBO_REF" = latest ]]; then
        # Wymagana wersja z meson.build mpv, np. ">=7.387.0" → najnowszy tag v7.x.y.
        need_pl="$(grep -oE "libplacebo', *version: *'>= *[0-9.]+" "$WORK/mpv/meson.build" | grep -oE '[0-9]+(\.[0-9]+)+$' || true)"
        pl_major="${need_pl%%.*}"
        LIBPLACEBO_REF="$({ GIT_TERMINAL_PROMPT=0 timeout 60 git ls-remote --tags --refs https://github.com/haasn/libplacebo.git 2>/dev/null || true; } \
            | sed 's|.*refs/tags/||' | { grep -E "^v${pl_major:-[0-9]+}\.[0-9]+\.[0-9]+\$" || true; } | sed 's/^v//' | sort -V | tail -n1)"
        [[ -n "$LIBPLACEBO_REF" ]] || fail "nie udało się ustalić wersji libplacebo (mpv wymaga ${need_pl:-?}) — podaj LIBPLACEBO_REF=..."
        echo "libplacebo $LIBPLACEBO_REF (mpv wymaga >= ${need_pl:-?})"
    fi
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "v$LIBPLACEBO_REF" \
        --recurse-submodules --shallow-submodules \
        https://github.com/haasn/libplacebo.git "$WORK/mpv/subprojects/libplacebo"
    meson setup "$WORK/mpv/build" "$WORK/mpv" --prefix="$MPVPREFIX" --buildtype=release \
        --force-fallback-for=libplacebo \
        -Dlibplacebo:demos=false -Dlibplacebo:tests=false -Dlibplacebo:vulkan=enabled \
        -Dlibplacebo:opengl=enabled -Dlibplacebo:shaderc=enabled -Dlibplacebo:lcms=enabled \
        -Dlibmpv=false -Dcplayer=true -Dc_args="-march=$TARGET_CPU"
    meson compile -C "$WORK/mpv/build" -j "$JOBS"
    meson install -C "$WORK/mpv/build" >/dev/null
fi
[[ -x "$MPVPREFIX/bin/mpv" ]] || fail "brak $MPVPREFIX/bin/mpv"
"$MPVPREFIX/bin/mpv" --no-config --version | sed -n 1p

# --------------------------------------------------------------- 3c. terminal (Alacritty)
# Okno z TUI otwierane z menu ma własną ikonę na pasku zadań i w ramce: Alacritty z podmienioną
# ikoną (X11) i klasą/app-id „videopipeline” (Wayland → videopipeline.desktop), pod inną nazwą pliku,
# żeby Plasma nie dopasowała procesu do alacritty.desktop.
if [[ "$ALACRITTY_VERSION" = latest ]]; then
    ALACRITTY_VERSION="$({ GIT_TERMINAL_PROMPT=0 timeout 60 git ls-remote --tags --refs https://github.com/alacritty/alacritty.git 2>/dev/null || true; } \
        | sed 's|.*refs/tags/||' | { grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true; } | sed 's/^v//' | sort -V | tail -n1)"
    if [[ -z "$ALACRITTY_VERSION" ]]; then
        ALACRITTY_VERSION="$(ls -1d "$CACHE"/alacritty-* 2>/dev/null | sed -E 's|.*/alacritty-([0-9.]+)-.*|\1|' | sort -V | tail -n1)"
    fi
    [[ -n "$ALACRITTY_VERSION" ]] || fail "nie udało się ustalić wersji Alacritty — podaj ALACRITTY_VERSION=..."
fi
log "3c/5 terminal: Alacritty $ALACRITTY_VERSION jako videopipeline-term (ze źródeł)"
ICON_TAG="$(sha256sum "$ROOT/icons/videopipeline-64.png" | cut -c1-8)"
TERMDIR="$CACHE/alacritty-$ALACRITTY_VERSION-$ICON_TAG"
if [[ "$FORCE" = 1 || ! -x "$TERMDIR/videopipeline-term" ]]; then
    rm -rf "$WORK/alacritty" "$TERMDIR"
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "v$ALACRITTY_VERSION" \
        https://github.com/alacritty/alacritty.git "$WORK/alacritty"
    # Ikona okna X11 jest wkompilowana (include_bytes!) — podmieniamy ją przed budowaniem.
    ICON_PNG="$WORK/alacritty/extra/logo/compat/alacritty-term.png"
    [[ -f "$ICON_PNG" ]] || fail "Alacritty $ALACRITTY_VERSION: brak $ICON_PNG (zmieniła się struktura źródeł)"
    cp "$ROOT/icons/videopipeline-64.png" "$ICON_PNG"
    # Własny katalog target i RUSTFLAGS: bez prefer-dynamic z .cargo/config.toml VideoPipeline.
    (cd "$WORK/alacritty" && CARGO_TARGET_DIR="$WORK/alacritty/target" RUSTFLAGS="-C target-cpu=$TARGET_CPU" \
        cargo build --release --locked -p alacritty)
    mkdir -p "$TERMDIR"
    install -m755 "$WORK/alacritty/target/release/alacritty" "$TERMDIR/videopipeline-term"
    strip "$TERMDIR/videopipeline-term"
    cp "$WORK/alacritty/LICENSE-APACHE" "$TERMDIR/LICENSE-ALACRITTY"
fi
"$TERMDIR/videopipeline-term" --version

# --------------------------------------------------------------------------- 4. AppDir
log "4/5 składanie AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/licenses/videopipeline" "$APPDIR/usr/share/misc"

install -m755 "$REL/videopipeline" "$APPDIR/usr/bin/"
install -m755 "$REL/libvideopipeline_core.so" "$APPDIR/usr/lib/"
# Rust dylib: binarka i biblioteka core potrzebują libstd z sysroot rustc.
SYSROOT="$(rustc --print sysroot)"
STD_SO="$(find "$SYSROOT/lib" -name 'libstd-*.so' | head -n1)"
[ -n "$STD_SO" ] || { echo "BŁĄD: brak libstd-*.so w $SYSROOT/lib"; exit 1; }
echo "libstd: $STD_SO"
cp "$STD_SO" "$APPDIR/usr/lib/"

install -m755 "$FFPREFIX/bin/ffmpeg" "$FFPREFIX/bin/ffprobe" "$APPDIR/usr/bin/"
install -m755 "$TOOLS/bin/dovi_tool" "$TOOLS/bin/hdr10plus_tool" "$APPDIR/usr/bin/"
install -m755 "$(command -v mkvmerge)" "$APPDIR/usr/bin/"
install -m755 "$(command -v mediainfo)" "$APPDIR/usr/bin/"
install -m755 "$MPVPREFIX/bin/mpv" "$APPDIR/usr/bin/"
install -m755 "$TERMDIR/videopipeline-term" "$APPDIR/usr/bin/"
install -m644 packaging/videopipeline-term.toml "$APPDIR/usr/bin/"
for m in /usr/lib/file/magic.mgc /usr/share/misc/magic.mgc; do
    if [[ -f "$m" ]]; then cp -L "$m" "$APPDIR/usr/share/misc/magic.mgc"; break; fi
done

install -m644 packaging/appimage/videopipeline.desktop "$APPDIR/usr/share/applications/"
install -m644 LICENSE "$APPDIR/usr/share/licenses/videopipeline/LICENSE"
install -m644 packaging/appimage/THIRD-PARTY-NOTICES.md "$APPDIR/usr/share/licenses/videopipeline/"
install -m644 "$TERMDIR/LICENSE-ALACRITTY" "$APPDIR/usr/share/licenses/videopipeline/"

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
    # Najpierw samo wdrożenie bibliotek (bez obrazu) — potem przenosimy część z nich do usr/lib/hostfirst.
    "$BIN/linuxdeploy-x86_64.AppImage" \
        --appdir "$APPDIR" \
        --executable "$APPDIR/usr/bin/videopipeline" \
        --executable "$APPDIR/usr/bin/mkvmerge" \
        --executable "$APPDIR/usr/bin/mediainfo" \
        --executable "$APPDIR/usr/bin/mpv" \
        --executable "$APPDIR/usr/bin/videopipeline-term" \
        --executable "$APPDIR/usr/bin/ffmpeg" \
        --executable "$APPDIR/usr/bin/ffprobe" \
        --executable "$APPDIR/usr/bin/dovi_tool" \
        --executable "$APPDIR/usr/bin/hdr10plus_tool" \
        --desktop-file "$APPDIR/usr/share/applications/videopipeline.desktop" \
        --icon-file "$ROOT/icons/videopipeline-256.png" --icon-filename videopipeline \
        --custom-apprun "$ROOT/packaging/appimage/AppRun"

    # Biblioteki, które mają pochodzić z systemu: libva/libdrm/oneVPL muszą pasować do sterownika iHD
    # i runtime'u oneVPL hosta, a libstdc++/libgcc_s do bibliotek hosta ładowanych przez te sterowniki.
    # Kopie trafiają do usr/lib/hostfirst; AppRun podsuwa je tylko wtedy, gdy w systemie ich brak.
    mkdir -p "$APPDIR/usr/lib/hostfirst"
    shopt -s nullglob
    for f in "$APPDIR"/usr/lib/{libva.so,libva-drm.so,libdrm.so,libvpl.so,libstdc++.so,libgcc_s.so}*; do
        mv -f "$f" "$APPDIR/usr/lib/hostfirst/"
    done
    shopt -u nullglob
    echo "hostfirst: $(ls "$APPDIR/usr/lib/hostfirst" | tr '\n' ' ')"

    "$BIN/linuxdeploy-plugin-appimage" --appdir "$APPDIR"
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
