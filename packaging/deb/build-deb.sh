#!/usr/bin/env bash
# Buduje dist/videopipeline_<wersja>_amd64.deb z drzewa AppDir przygotowanego przez build-appimage.sh
# (po linuxdeploy: binarki z dołączonymi bibliotekami i rpath $ORIGIN/../lib).
# Instalacja: /opt/videopipeline/{bin,lib,share}, polecenie /usr/bin/videopipeline, wpis w menu i ikony.
# Sterownik NVIDIA (i VA-API dla Intela) zostają po stronie systemu.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="${VERSION:-$(grep -m1 '^version' app/Cargo.toml | cut -d'"' -f2)}"
APPDIR="$ROOT/build/work/AppDir"
DIST="$ROOT/dist"
PKG="$ROOT/build/work/deb/videopipeline_${VERSION}_amd64"
OPT="$PKG/opt/videopipeline"

log()  { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mBŁĄD: %s\033[0m\n' "$*" >&2; exit 1; }

[[ -x "$APPDIR/usr/bin/videopipeline" ]] || fail "brak $APPDIR — najpierw packaging/appimage/build-appimage.sh"
command -v dpkg-deb >/dev/null || fail "brak dpkg-deb"

log "deb: składanie pakietu $VERSION"
rm -rf "$PKG"
mkdir -p "$OPT" "$PKG/DEBIAN" "$PKG/usr/bin" "$PKG/usr/share/applications" \
         "$PKG/usr/share/icons/hicolor/scalable/apps" "$PKG/usr/share/doc/videopipeline"

cp -a "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$OPT/"
# W pakiecie deb libva/libdrm/oneVPL/libstdc++ zawsze z systemu (Depends niżej) — bez kopii z AppImage.
INTEL_DEPS=""
if compgen -G "$OPT/lib/hostfirst/libva.so*" >/dev/null; then
    INTEL_DEPS=", libva2, libva-drm2, libdrm2, libvpl2"
fi
rm -rf "$OPT/lib/hostfirst"
mkdir -p "$OPT/share"
[[ -d "$APPDIR/usr/share/misc" ]] && cp -a "$APPDIR/usr/share/misc" "$OPT/share/"
cp -a "$APPDIR/usr/share/licenses/videopipeline/." "$PKG/usr/share/doc/videopipeline/"

# Polecenie w PATH: ustawia bazę typów plików dla dołączonego mkvmerge (libmagic), reszta jest w rpath/PATH aplikacji.
cat > "$PKG/usr/bin/videopipeline" <<'W'
#!/bin/sh
[ -f /opt/videopipeline/share/misc/magic.mgc ] && export MAGIC=/opt/videopipeline/share/misc/magic.mgc
exec /opt/videopipeline/bin/videopipeline "$@"
W
chmod 755 "$PKG/usr/bin/videopipeline"

sed 's|^Exec=.*|Exec=videopipeline|' packaging/appimage/videopipeline.desktop \
    > "$PKG/usr/share/applications/videopipeline.desktop"
chmod 644 "$PKG/usr/share/applications/videopipeline.desktop"
install -m644 icons/videopipeline.svg "$PKG/usr/share/icons/hicolor/scalable/apps/videopipeline.svg"
for sz in 32 48 64 128 256; do
    d="$PKG/usr/share/icons/hicolor/${sz}x${sz}/apps"
    mkdir -p "$d"
    install -m644 "icons/videopipeline-$sz.png" "$d/videopipeline.png"
done

# Bez zbędnych plików (rozwojowych); uprawnienia katalogów.
find "$PKG" -type d -exec chmod 755 {} +
find "$OPT" -type f -name '*.a' -delete

SIZE="$(du -sk "$PKG" | cut -f1)"
cat > "$PKG/DEBIAN/control" <<C
Package: videopipeline
Version: ${VERSION}-1
Section: video
Priority: optional
Architecture: amd64
Installed-Size: ${SIZE}
Depends: libc6 (>= 2.41), libstdc++6, libgcc-s1${INTEL_DEPS}
Recommends: intel-media-va-driver-non-free | intel-media-va-driver, libmfx-gen1.2
Suggests: nvidia-driver
Maintainer: VideoPipeline <noreply@users.noreply.github.com>
Homepage: https://github.com/8brzrs26ws-cloud/videopipeline
Description: Kodowanie wideo HEVC/AV1 na GPU (NVIDIA NVENC, Intel QSV/VAAPI) z TUI w stylu MC
 Zawiera własne kopie ffmpeg (NVENC/NVDEC/CUDA, Intel VAAPI/QSV), ffprobe, mkvmerge,
 dovi_tool i hdr10plus_tool w /opt/videopipeline. Wymaga sterownika GPU z systemu
 oraz procesora x86-64-v3 (Haswell / Zen 1 i nowsze).
C

mkdir -p "$DIST"
OUT="$DIST/videopipeline_${VERSION}_amd64.deb"
rm -f "$OUT"
dpkg-deb --root-owner-group -Zxz --build "$PKG" "$OUT" >/dev/null
(cd "$DIST" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
dpkg-deb -I "$OUT" | sed -n 1,12p
log "Gotowe: $OUT ($(du -h "$OUT" | cut -f1))"
