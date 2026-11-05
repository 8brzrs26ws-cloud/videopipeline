#!/usr/bin/env bash
# Buduje binarkę `videopipeline` i bibliotekę dynamiczną `libvideopipeline_core.so`.
# Biblioteka Rusta (dylib) wymaga, by std było linkowane dynamicznie (-C prefer-dynamic),
# dlatego binarka dostaje rpath: obok siebie ($ORIGIN), ../lib oraz sysroot rustc (libstd).
set -euo pipefail
cd "$(dirname "$0")"
SYSROOT="$(rustc --print sysroot)"
export RUSTFLAGS="-C prefer-dynamic -C link-arg=-Wl,-rpath,\$ORIGIN -C link-arg=-Wl,-rpath,\$ORIGIN/../lib -C link-arg=-Wl,-rpath,${SYSROOT}/lib"
cargo build --release "$@"
echo
echo "Gotowe: target/release/videopipeline + target/release/libvideopipeline_core.so"
echo "Przy kopiowaniu zabierz oba pliki do tego samego katalogu."
