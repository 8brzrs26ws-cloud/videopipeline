#!/usr/bin/env bash
# Uruchamiane WEWNĄTRZ kontenera (docker/Dockerfile).
# TARGETS: "linux" (AppImage + deb z tego samego drzewa plików), "windows" (.zip), domyślnie oba.
set -euo pipefail
mkdir -p "$HOME"
cd /src
git config --global --add safe.directory /src 2>/dev/null || true
TARGETS="${TARGETS:-linux windows}"
if [[ " $TARGETS " == *" linux "* ]]; then
    bash packaging/appimage/build-appimage.sh
    bash packaging/deb/build-deb.sh
fi
if [[ " $TARGETS " == *" windows "* ]]; then
    bash packaging/windows/build-windows.sh
fi
