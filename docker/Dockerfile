# Środowisko budowania wydań: AppImage + deb (Debian 13 "trixie", glibc 2.41)
# oraz kroskompilacja Windows x64 (MinGW-w64).
# Obraz zawiera tylko narzędzia; kod, cache i wyniki są montowane z hosta (patrz build-local.sh).
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential nasm yasm pkg-config clang llvm lld patchelf file wget curl git \
        ca-certificates xz-utils zstd dpkg-dev fakeroot desktop-file-utils squashfs-tools \
        mkvtoolnix mediainfo libdav1d-dev zlib1g-dev libmagic-mgc \
        libva-dev libdrm-dev libvpl-dev \
        mingw-w64 libz-mingw-w64-dev meson ninja-build cmake 7zip zip libarchive-tools \
        libshaderc-dev libass-dev libavcodec-dev libavformat-dev libavfilter-dev libavdevice-dev \
        libavutil-dev libswscale-dev libswresample-dev liblua5.2-dev libuchardet-dev liblcms2-dev \
        libjpeg-dev libzimg-dev libx11-dev libxext-dev libxpresent-dev libxrandr-dev libxinerama-dev \
        libxss-dev libxkbcommon-dev libwayland-dev wayland-protocols libegl-dev libgl-dev libgbm-dev \
        libvulkan-dev libasound2-dev libpulse-dev python3 \
    && rm -rf /var/lib/apt/lists/*

# Rust (stable) w katalogu dostępnym dla każdego użytkownika; CARGO_HOME ustawia build-local.sh
# na katalog w repozytorium, żeby cache crate'ów przetrwał między uruchomieniami.
ENV RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/opt/cargo sh -s -- -y --profile minimal --default-toolchain stable \
    && CARGO_HOME=/opt/cargo rustup target add x86_64-pc-windows-gnu \
    && chmod -R a+rX /opt/rustup /opt/cargo

WORKDIR /src
