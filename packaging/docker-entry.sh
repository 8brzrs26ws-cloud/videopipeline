#!/usr/bin/env bash
# Uruchamiane WEWNĄTRZ kontenera (docker/Dockerfile): AppImage, potem deb z tego samego drzewa plików.
set -euo pipefail
mkdir -p "$HOME"
cd /src
git config --global --add safe.directory /src 2>/dev/null || true
bash packaging/appimage/build-appimage.sh
bash packaging/deb/build-deb.sh
