# Środowisko budowania wydań (AppImage + deb): Debian 13 "trixie" (glibc 2.41).
# Obraz zawiera tylko narzędzia; kod, cache i wyniki są montowane z hosta (patrz build-local.sh).
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential nasm yasm pkg-config clang llvm lld patchelf file wget curl git \
        ca-certificates xz-utils zstd dpkg-dev fakeroot desktop-file-utils squashfs-tools \
        mkvtoolnix libdav1d-dev zlib1g-dev libmagic-mgc \
        libva-dev libdrm-dev libvpl-dev \
    && rm -rf /var/lib/apt/lists/*

# Rust (stable) w katalogu dostępnym dla każdego użytkownika; CARGO_HOME ustawia build-local.sh
# na katalog w repozytorium, żeby cache crate'ów przetrwał między uruchomieniami.
ENV RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/opt/cargo sh -s -- -y --profile minimal --default-toolchain stable \
    && chmod -R a+rX /opt/rustup /opt/cargo

WORKDIR /src
