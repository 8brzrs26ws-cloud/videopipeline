#!/usr/bin/env bash
# Buduje lokalnie w Dockerze (Debian 13, glibc 2.41) wszystko do wydania:
#   dist/VideoPipeline-<wersja>-x86_64_v3.AppImage  (+ .sha256)
#   dist/videopipeline_<wersja>_amd64.deb
# Cache (ffmpeg, dovi_tool, hdr10plus_tool, crate'y) zostaje w ./build, więc kolejne budowania są szybkie.
#
# Zmienne (przekazywane do kontenera): FFMPEG_VERSION (domyślnie latest), DOVI_TOOL_REF, HDR10PLUS_TOOL_REF, NVCODEC_HEADERS_REF
#   — wartość "latest" bierze najnowsze wydanie; INTEL=auto|1|0 (VAAPI/QSV w ffmpeg); TARGET_CPU; FORCE=1.
# Silnik: docker lub podman (DOCKER=podman).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

ENGINE="${DOCKER:-docker}"
command -v "$ENGINE" >/dev/null || { echo "BŁĄD: brak polecenia $ENGINE" >&2; exit 1; }
IMAGE="videopipeline-build:trixie"

if ! "$ENGINE" info >/dev/null 2>&1; then
    echo "BŁĄD: $ENGINE nie odpowiada (demon nie działa albo brak uprawnień)." >&2
    echo "  Nie zainstalowany:      ./install-build-deps.sh" >&2
    echo "  Demon wyłączony:        sudo systemctl enable --now docker" >&2
    echo "  Brak uprawnień (grupa): sudo usermod -aG docker \$USER  i ponowne zalogowanie" >&2
    echo "  Alternatywa bez demona: DOCKER=podman ./build-local.sh" >&2
    exit 1
fi

mkdir -p build dist
# dist/ zawiera tylko wynik bieżącego budowania (starsze wersje są na GitHub Releases).
rm -f dist/*.AppImage dist/*.deb dist/*.sha256
"$ENGINE" build -t "$IMAGE" docker

# Domyślnie najnowszy stabilny ffmpeg (nadpisz np. FFMPEG_VERSION=7.1.1 dla powtarzalnego wydania).
export FFMPEG_VERSION="${FFMPEG_VERSION:-latest}"

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
