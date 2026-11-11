#!/usr/bin/env bash
# Buduje binarkę `videopipeline` i bibliotekę dynamiczną `libvideopipeline_core.so`.
# Biblioteka Rusta (dylib) wymaga, by std było linkowane dynamicznie (-C prefer-dynamic),
# dlatego binarka dostaje rpath: obok siebie ($ORIGIN), ../lib oraz sysroot rustc (libstd).
# Dodatkowe flagi rustc: VP_RUSTFLAGS_EXTRA="-C target-cpu=x86-64-v3" ./build.sh
set -euo pipefail
cd "$(dirname "$0")"
# Wydania (AppImage, deb, Arch, Windows .zip) buduje build-local.sh w Dockerze — tu tylko wersja deweloperska.
case "${1:-}" in
    linux|windows)
        echo "./build.sh to wersja deweloperska (Rust na tym komputerze). Paczki $1 buduje: ./build-local.sh $1"
        exec ./build-local.sh "$@" ;;
esac
command -v rustc >/dev/null || {
    echo "Brak Rusta (rustc). Wersja deweloperska: ./install-build-deps.sh --dev" >&2
    echo "Paczki (bez Rusta na hoście, w Dockerze): ./build-local.sh [linux|windows]" >&2
    exit 1
}
SYSROOT="$(rustc --print sysroot)"
export RUSTFLAGS="-C prefer-dynamic -C link-arg=-Wl,-rpath,\$ORIGIN -C link-arg=-Wl,-rpath,\$ORIGIN/../lib -C link-arg=-Wl,-rpath,${SYSROOT}/lib -C link-arg=-Wl,-rpath,${SYSROOT}/lib/rustlib/x86_64-unknown-linux-gnu/lib ${VP_RUSTFLAGS_EXTRA:-}"
cargo build --release "$@"
echo
OUT="${CARGO_TARGET_DIR:-target}/release"
echo "Gotowe: $OUT/videopipeline + $OUT/libvideopipeline_core.so"
echo "Przy kopiowaniu zabierz oba pliki do tego samego katalogu."
