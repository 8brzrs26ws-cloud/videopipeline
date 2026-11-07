#!/usr/bin/env bash
# Instaluje to, czego potrzebuje ./build-local.sh i ./release.sh na hoście: Docker (z buildx), git, GitHub CLI.
# Cała reszta (Rust, ffmpeg, clang, nasm...) jest w obrazie Dockera, więc na hoście nie jest potrzebna.
# Obsługa: Arch/CachyOS/Manjaro (pacman) oraz Debian/Ubuntu (apt).
set -euo pipefail

. /etc/os-release
ID_ALL=" ${ID:-} ${ID_LIKE:-} "
SUDO=""; [[ $EUID -ne 0 ]] && SUDO="sudo"

case "$ID_ALL" in
    *" arch "*)
        $SUDO pacman -S --needed --noconfirm docker docker-buildx git github-cli
        ;;
    *" debian "*|*" ubuntu "*)
        $SUDO apt-get update
        # docker.io + buildx z repozytoriów dystrybucji (Debian 13 / Ubuntu 24.04+); gh w Debianie 13 jest w repo.
        $SUDO apt-get install -y --no-install-recommends docker.io docker-buildx git gh ca-certificates
        ;;
    *)
        echo "Nieobsługiwana dystrybucja ($PRETTY_NAME). Zainstaluj ręcznie: docker (+buildx), git, gh." >&2
        exit 1
        ;;
esac

$SUDO systemctl enable --now docker
if ! id -nG "$USER" | grep -qw docker; then
    $SUDO usermod -aG docker "$USER"
    echo
    echo "Dodano $USER do grupy docker — WYLOGUJ SIĘ i zaloguj ponownie (albo: newgrp docker), potem ./build-local.sh"
fi
echo
echo "Gotowe. Dalej:  gh auth login && gh auth setup-git   (raz, do pushowania i ./release.sh)"
