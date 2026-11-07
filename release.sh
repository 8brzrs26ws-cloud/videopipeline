#!/usr/bin/env bash
# Wysyła zawartość dist/ jako wydanie na GitHubie:  ./release.sh v0.12.0
# Wymaga zalogowanego `gh` (gh auth login). Istniejące wydanie dostaje pliki podmienione (--clobber).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
TAG="${1:?podaj tag, np. ./release.sh v0.12.0}"
shopt -s nullglob
FILES=(dist/*.AppImage dist/*.deb dist/*.sha256)
[[ ${#FILES[@]} -gt 0 ]] || { echo "BŁĄD: dist/ jest pusty — najpierw ./build-local.sh" >&2; exit 1; }
if gh release view "$TAG" >/dev/null 2>&1; then
    gh release upload "$TAG" "${FILES[@]}" --clobber
else
    gh release create "$TAG" "${FILES[@]}" --title "VideoPipeline ${TAG#v}" --generate-notes
fi
echo "Gotowe: $(gh release view "$TAG" --json url -q .url)"
