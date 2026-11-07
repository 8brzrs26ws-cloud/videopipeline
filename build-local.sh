#!/usr/bin/env bash
# Buduje lokalnie w Dockerze (Debian 13, glibc 2.41) wszystko do wydania:
#   dist/VideoPipeline-<wersja>-x86_64_v3.AppImage  (+ .sha256)
#   dist/videopipeline_<wersja>_amd64.deb
# Cache (ffmpeg, dovi_tool, hdr10plus_tool, crate'y) zostaje w ./build, więc kolejne budowania są szybkie.
#
# Zmienne (przekazywane do kontenera): FFMPEG_VERSION, DOVI_TOOL_REF, HDR10PLUS_TOOL_REF, NVCODEC_HEADERS_REF
#   — wartość "latest" bierze najnowsze wydanie; INTEL=auto|1|0 (VAAPI/QSV w ffmpeg); TARGET_CPU; FORCE=1.
# Silnik: docker lub podman (DOCKER=podman).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

ENGINE="${DOCKER:-docker}"
command -v "$ENGINE" >/dev/null || { echo "BŁĄD: brak polecenia $ENGINE" >&2; exit 1; }
IMAGE="videopipeline-build:trixie"

mkdir -p build dist
"$ENGINE" build -t "$IMAGE" docker

PASS=()
for v in FFMPEG_VERSION DOVI_TOOL_REF HDR10PLUS_TOOL_REF NVCODEC_HEADERS_REF INTEL TARGET_CPU FORCE JOBS VERSION; do
    [[ -n "${!v:-}" ]] && PASS+=(-e "$v=${!v}")
done

"$ENGINE" run --rm \
    --user "$(id -u):$(id -g)" \
    -e HOME=/tmp/home -e CARGO_HOME=/src/build/cargo-home -e APPIMAGE_EXTRACT_AND_RUN=1 \
    "${PASS[@]}" \
    -v "$PWD:/src" \
    "$IMAGE" bash packaging/docker-entry.sh

echo
ls -lh dist/
