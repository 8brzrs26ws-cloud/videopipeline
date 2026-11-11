#!/usr/bin/env bash
# Instaluje to, czego potrzebuje ./build-local.sh i ./release.sh na hoście: Docker (z buildx), git, GitHub CLI.
# Cała reszta (Rust, ffmpeg, clang, nasm...) jest w obrazie Dockera, więc na hoście nie jest potrzebna.
#   --dev   dodatkowo wersja deweloperska (./build.sh): Rust (rustup) oraz ffmpeg, mkvtoolnix, mediainfo, mpv
# Obsługa: Arch/CachyOS/Manjaro (pacman) oraz Debian/Ubuntu (apt), także w WSL.
set -euo pipefail
DEV=0
[[ "${1:-}" = --dev ]] && DEV=1

# WSL: Docker potrzebuje systemd.
if grep -qi microsoft /proc/version 2>/dev/null && [[ "$(ps -p 1 -o comm= 2>/dev/null)" != systemd ]]; then
    echo "WSL bez systemd — Docker nie wystartuje. Włącz systemd:" >&2
    echo "  printf '[boot]\nsystemd=true\n' | sudo tee /etc/wsl.conf" >&2
    echo "potem w PowerShellu: wsl --shutdown, otwórz dystrybucję ponownie i uruchom ten skrypt jeszcze raz." >&2
    exit 1
fi

. /etc/os-release
ID_ALL=" ${ID:-} ${ID_LIKE:-} "
SUDO=""; [[ $EUID -ne 0 ]] && SUDO="sudo"

case "$ID_ALL" in
    *" arch "*)
        $SUDO pacman -S --needed --noconfirm docker docker-buildx git github-cli
        [[ $DEV = 1 ]] && $SUDO pacman -S --needed --noconfirm rustup base-devel ffmpeg mkvtoolnix-cli mediainfo mpv
        ;;
    *" debian "*|*" ubuntu "*)
        $SUDO apt-get update
        # docker.io + buildx z repozytoriów dystrybucji (Debian 13 / Ubuntu 24.04+); gh w Debianie 13 jest w repo.
        # Debian 13: klient (polecenie docker) jest w osobnym pakiecie docker-cli; starsze wydania go nie mają.
        CLI=""; apt-cache show docker-cli >/dev/null 2>&1 && CLI="docker-cli"
        $SUDO apt-get install -y --no-install-recommends docker.io $CLI docker-buildx git gh ca-certificates
        [[ $DEV = 1 ]] && $SUDO apt-get install -y --no-install-recommends build-essential curl pkg-config \
            ffmpeg mkvtoolnix mediainfo mpv
        ;;
    *)
        echo "Nieobsługiwana dystrybucja ($PRETTY_NAME). Zainstaluj ręcznie: docker (+buildx), git, gh." >&2
        exit 1
        ;;
esac

$SUDO systemctl enable --now docker

# Rust dla ./build.sh (rustup w katalogu użytkownika). dovi_tool i hdr10plus_tool: cargo install.
if [[ $DEV = 1 ]]; then
    if command -v rustup >/dev/null; then
        rustup default stable >/dev/null 2>&1 || rustup toolchain install stable
    elif ! command -v rustc >/dev/null; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        . "$HOME/.cargo/env"
    fi
    echo "Rust: $(rustc --version 2>/dev/null || echo 'otwórz nowy terminal (PATH z ~/.cargo/bin)')"
    echo "dovi_tool i hdr10plus_tool (opcjonalnie, dla DV/HDR10+):"
    echo "  cargo install --git https://github.com/quietvoid/dovi_tool dovi_tool"
    echo "  cargo install --git https://github.com/quietvoid/hdr10plus_tool hdr10plus_tool"
fi
if ! id -nG "$USER" | grep -qw docker; then
    $SUDO usermod -aG docker "$USER"
    echo
    echo "Dodano $USER do grupy docker — WYLOGUJ SIĘ i zaloguj ponownie (albo: newgrp docker), potem ./build-local.sh"
fi
echo
echo "Gotowe. Dalej:  gh auth login && gh auth setup-git   (raz, do pushowania i ./release.sh)"
