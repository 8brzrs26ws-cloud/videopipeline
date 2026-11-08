#!/usr/bin/env bash
# Uruchamiane WEWNĄTRZ kontenera (docker/Dockerfile).
# TARGETS: "linux" (AppImage, deb i pakiet Arch z tego samego drzewa plików), "windows" (.zip), domyślnie oba.
set -euo pipefail
trap 'echo "BŁĄD: budowanie przerwane (linia $LINENO: $BASH_COMMAND)" >&2' ERR
mkdir -p "$HOME"
cd /src
git config --global --add safe.directory /src 2>/dev/null || true
# Osobny katalog kompilacji kontenera: target/ zostaje dla buildów na hoście (./build.sh, ./install.sh).
# Binarka z kontenera ma rpath do toolchaina w kontenerze i nie uruchomi się wprost na hoście.
export CARGO_TARGET_DIR=/src/build/target
TARGETS="${TARGETS:-linux windows}"
if [[ " $TARGETS " == *" linux "* ]]; then
    bash packaging/appimage/build-appimage.sh
    bash packaging/deb/build-deb.sh
    bash packaging/arch/build-arch.sh
fi
if [[ " $TARGETS " == *" windows "* ]]; then
    bash packaging/windows/build-windows.sh
fi
echo "Zbudowane cele: $TARGETS"
