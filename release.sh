#!/usr/bin/env bash
# Wysyła pliki BIEŻĄCEJ wersji z dist/ jako wydanie na GitHubie:  ./release.sh v0.17.0
# Bierze tylko pliki z numerem wersji z tagu (starsze buildy leżące w dist/ są pomijane),
# a tag musi zgadzać się z wersją w app/Cargo.toml.
# Wymaga zalogowanego `gh` (gh auth login). Istniejące wydanie dostaje pliki podmienione (--clobber),
# a pliki innych wersji są z niego usuwane.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
TAG="${1:?podaj tag, np. ./release.sh v0.17.0}"
VER="${TAG#v}"
CARGO_VER="$(grep -m1 '^version' app/Cargo.toml | cut -d'"' -f2)"
[[ "$VER" = "$CARGO_VER" ]] || { echo "BŁĄD: tag $TAG nie zgadza się z wersją w app/Cargo.toml ($CARGO_VER)" >&2; exit 1; }

shopt -s nullglob
FILES=(dist/VideoPipeline-"$VER"-*.AppImage dist/VideoPipeline-"$VER"-*.AppImage.sha256
       dist/videopipeline_"$VER"_*.deb dist/videopipeline_"$VER"_*.deb.sha256
       dist/videopipeline-"$VER"-*-x86_64.pkg.tar.zst dist/videopipeline-"$VER"-*-x86_64.pkg.tar.zst.sha256
       dist/VideoPipeline-"$VER"-windows-*.zip dist/VideoPipeline-"$VER"-windows-*.zip.sha256)
[[ ${#FILES[@]} -gt 0 ]] || { echo "BŁĄD: brak plików wersji $VER w dist/ — najpierw ./build-local.sh" >&2; exit 1; }
printf 'Wysyłam:\n'; printf '  %s\n' "${FILES[@]}"

if gh release view "$TAG" >/dev/null 2>&1; then
    gh release upload "$TAG" "${FILES[@]}" --clobber
else
    gh release create "$TAG" "${FILES[@]}" --title "VideoPipeline $VER" --generate-notes
fi
# Wydanie ma zawierać tylko pliki tej wersji — usuń pozostałości innych (np. po zmianie tagu wydania).
while read -r asset; do
    [[ -z "$asset" || "$asset" == *"$VER"* ]] && continue
    echo "Usuwam plik innej wersji: $asset"
    gh release delete-asset "$TAG" "$asset" --yes
done < <(gh release view "$TAG" --json assets --jq '.assets[].name')
echo "Gotowe: $(gh release view "$TAG" --json url -q .url)"
