#!/usr/bin/env bash
# Usuwa VideoPipeline zainstalowany przez ./install.sh (domyślnie z ~/.local):
# binarkę, biblioteki, ikony i wpis w menu. Pakiety systemowe (pacman/apt) usuwa się menedżerem pakietów.
#   ./uninstall.sh            # usuń instalację z ~/.local
#   PREFIX=/inny/katalog ./uninstall.sh
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
ICONS="$PREFIX/share/icons/hicolor"
removed=0

rm_if() {
    for f in "$@"; do
        if [[ -e "$f" || -L "$f" ]]; then
            rm -f "$f"
            echo "usunięto: $f"
            removed=1
        fi
    done
}

# Reguła okna KWin (ikona na pasku zadań KDE) — usuwa ją sam program, zanim zniknie.
if [[ -x "$PREFIX/bin/videopipeline" ]]; then
    "$PREFIX/bin/videopipeline" --kde-icon off 2>/dev/null | grep -F "Usunięto" || true
    rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/videopipeline/no-kde-taskbar-rule"
    rmdir "${XDG_CONFIG_HOME:-$HOME/.config}/videopipeline" 2>/dev/null || true
fi

rm_if "$PREFIX/bin/videopipeline" "$PREFIX/bin/libvideopipeline_core.so"
# libstd Rusta kopiowana przez install.sh obok binarki — tylko gdy nic innego z Rusta jej tam nie używa.
shopt -s nullglob
for f in "$PREFIX"/bin/libstd-*.so; do
    users=0
    for exe in "$PREFIX"/bin/*; do
        [[ -f "$exe" && "$exe" != "$f" ]] || continue
        if readelf -d "$exe" 2>/dev/null | grep -F "$(basename "$f")" >/dev/null; then users=1; break; fi
    done
    if [[ $users = 0 ]]; then rm_if "$f"; else echo "zostawiono $f (używa go inny program w $PREFIX/bin)"; fi
done
shopt -u nullglob

rm_if "$PREFIX/share/applications/videopipeline.desktop" "$ICONS/scalable/apps/videopipeline.svg"
for n in 32 48 64 128 256; do
    rm_if "$ICONS/${n}x${n}/apps/videopipeline.png"
done

command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$ICONS" >/dev/null 2>&1 || true
command -v update-desktop-database >/dev/null && update-desktop-database "$PREFIX/share/applications" >/dev/null 2>&1 || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true

if [[ $removed = 1 ]]; then
    echo "VideoPipeline usunięty z $PREFIX."
else
    echo "Nie znaleziono instalacji w $PREFIX."
fi

# Instalacje z pakietów — tylko podpowiedź, bez sudo w tle.
if command -v pacman >/dev/null && pacman -Q videopipeline >/dev/null 2>&1; then
    echo "Zainstalowany jest też pakiet systemowy: sudo pacman -R videopipeline"
fi
if command -v dpkg-query >/dev/null && dpkg-query -W -f='${Status}' videopipeline 2>/dev/null | grep -q "install ok installed"; then
    echo "Zainstalowany jest też pakiet systemowy: sudo apt remove videopipeline"
fi
echo "Pliki wynikowe kodowania (np. ~/Wideo) i AppImage nie są usuwane."
