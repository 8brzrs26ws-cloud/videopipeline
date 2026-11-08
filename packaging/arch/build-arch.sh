#!/usr/bin/env bash
# Buduje pakiet dla Arch Linux / CachyOS: dist/videopipeline-<wersja>-1-x86_64.pkg.tar.zst
# z tego samego drzewa co pakiet deb (packaging/deb/build-deb.sh musi być uruchomiony wcześniej).
# Kontener to Debian (bez makepkg), więc pakiet składamy tak jak makepkg: .PKGINFO + .MTREE
# + pliki, archiwum bsdtar skompresowane zstd. Instalacja: sudo pacman -U <plik>.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="${VERSION:-$(grep -m1 '^version' app/Cargo.toml | cut -d'"' -f2)}"
PKGREL="${PKGREL:-1}"
DEBTREE="$ROOT/build/work/deb/videopipeline_${VERSION}_amd64"
PKG="$ROOT/build/work/arch/videopipeline-${VERSION}"
DIST="$ROOT/dist"

log()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mBŁĄD [Arch]: %s\033[0m\n' "$*" >&2; exit 1; }
trap 'fail "przerwano w linii $LINENO: $BASH_COMMAND"' ERR

[[ -d "$DEBTREE/opt/videopipeline" ]] || fail "brak $DEBTREE — najpierw packaging/deb/build-deb.sh"
for c in bsdtar zstd gzip; do command -v "$c" >/dev/null || fail "brak polecenia $c"; done

log "Arch: składanie pakietu $VERSION-$PKGREL"
rm -rf "$PKG"
mkdir -p "$PKG"
cp -a "$DEBTREE/." "$PKG/"
rm -rf "$PKG/DEBIAN"
# Konwencja Archa: licencje w /usr/share/licenses/<pakiet>.
if [[ -d "$PKG/usr/share/doc/videopipeline" ]]; then
    mkdir -p "$PKG/usr/share/licenses"
    mv "$PKG/usr/share/doc/videopipeline" "$PKG/usr/share/licenses/videopipeline"
    rmdir "$PKG/usr/share/doc" 2>/dev/null || true
fi

# Zależności: jak w deb — libva/libdrm/oneVPL tylko gdy ffmpeg zbudowano z Intelem (deb ma je w Depends).
DEPENDS=("glibc>=2.41" "gcc-libs")
if grep -q '^Depends:.*libvpl2' "$DEBTREE/DEBIAN/control"; then
    DEPENDS+=("libva" "libdrm" "libvpl")
fi

SIZE_BYTES=$(( $(du -sk --apparent-size "$PKG" | cut -f1) * 1024 ))
{
    echo "# Wygenerowane przez packaging/arch/build-arch.sh"
    echo "pkgname = videopipeline"
    echo "pkgbase = videopipeline"
    echo "pkgver = $VERSION-$PKGREL"
    echo "pkgdesc = Kodowanie wideo HEVC/AV1 na GPU (NVIDIA NVENC, Intel QSV/VAAPI) z TUI w stylu Midnight Commandera"
    echo "url = https://github.com/8brzrs26ws-cloud/videopipeline"
    echo "builddate = $(date +%s)"
    echo "packager = VideoPipeline build (build-local.sh)"
    echo "size = $SIZE_BYTES"
    echo "arch = x86_64"
    echo "license = GPL-3.0-or-later"
    for d in "${DEPENDS[@]}"; do echo "depend = $d"; done
    echo "optdepend = nvidia-utils: kodowanie NVENC i monitor obciążenia (nvidia-smi)"
    echo "optdepend = intel-media-driver: kodowanie na karcie Intel (VA-API, sterownik iHD)"
    echo "optdepend = vpl-gpu-rt: Intel QSV (runtime oneVPL, Tiger Lake i nowsze)"
    echo "optdepend = intel-media-sdk: Intel QSV na starszych kartach (do Ice Lake)"
} > "$PKG/.PKGINFO"

(
    cd "$PKG"
    # Pełna lista ścieżek (bsdtar -n nie schodzi rekurencyjnie) — jak list_package_files w makepkg.
    list() { find . -mindepth 1 ! -path ./.MTREE -print0 | LC_ALL=C sort -z | sed -z 's|^\./||'; }
    # Właściciel plików: root (jak w makepkg/fakeroot). Opcje .MTREE — jak w makepkg.
    list | LANG=C bsdtar -cnf - --format=mtree --uid 0 --gid 0 --uname root --gname root \
        --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' \
        --null --files-from - | gzip -c -f -n > .MTREE
    mkdir -p "$DIST"
    OUT="$DIST/videopipeline-$VERSION-$PKGREL-x86_64.pkg.tar.zst"
    rm -f "$OUT"
    { printf '.MTREE\0'; list; } | LANG=C bsdtar --no-fflags -cnf - --uid 0 --gid 0 --uname root --gname root \
        --null --files-from - | zstd -q -c -T0 -19 > "$OUT"
    (cd "$DIST" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256")
    bsdtar -tf "$OUT" | sed -n 1,6p
    log "Gotowe: $OUT ($(du -h "$OUT" | cut -f1))"
)
