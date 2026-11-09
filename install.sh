#!/usr/bin/env bash
# Instaluje videopipeline do ~/.local (binarka + biblioteka + ikona) i dodaje wpis do menu KDE.
# Uruchomienie z menu: program sam otwiera okno terminala z tytułem/klasą „videopipeline”.
set -euo pipefail
cd "$(dirname "$0")"

BIN=target/release/videopipeline
LIB=target/release/libvideopipeline_core.so
[[ -x $BIN && -f $LIB ]] || { echo "Najpierw zbuduj projekt: ./build.sh" >&2; exit 1; }

PREFIX="${PREFIX:-$HOME/.local}"
ICONS="$PREFIX/share/icons/hicolor"
mkdir -p "$PREFIX/bin" "$PREFIX/share/applications" "$ICONS/scalable/apps"

# libstd Rusta, z którą zbudowano binarkę (nazwa z hashem toolchaina). Kopiujemy ją obok binarki,
# żeby instalacja przetrwała `rustup update` i nie zależała od toolchaina.
STD="$(readelf -d "$LIB" 2>/dev/null | sed -n 's/.*Shared library: \[\(libstd-[^]]*\.so\)\].*/\1/p' | head -n1)"
[[ -n "$STD" ]] || { echo "Nie udało się ustalić libstd binarki (readelf z binutils?)" >&2; exit 1; }
SYSROOT="$(rustc --print sysroot 2>/dev/null || true)"
STD_PATH="$(find "$SYSROOT/lib" -name "$STD" 2>/dev/null | head -n1)"
if [[ -z "$STD_PATH" ]]; then
  echo "Binarka wymaga $STD, którego nie ma w obecnym toolchainie Rusta." >&2
  echo "Prawdopodobnie target/ pochodzi z innego toolchaina (np. starsza wersja Rusta albo kontener)." >&2
  echo "Przebuduj na tym komputerze: ./build.sh  i ponów ./install.sh" >&2
  exit 1
fi

# Biblioteki leżą obok binarki — rpath $ORIGIN z build.sh je znajdzie.
install -m 755 "$BIN" "$PREFIX/bin/videopipeline"
install -m 755 "$LIB" "$PREFIX/bin/libvideopipeline_core.so"
rm -f "$PREFIX"/bin/libstd-*.so
install -m 644 "$STD_PATH" "$PREFIX/bin/$STD"

# Ikona: SVG (skalowalna) + PNG 48/64/128/256.
install -m 644 icons/videopipeline.svg "$ICONS/scalable/apps/videopipeline.svg"
for n in 32 48 64 128 256; do
  mkdir -p "$ICONS/${n}x${n}/apps"
  install -m 644 "icons/videopipeline-$n.png" "$ICONS/${n}x${n}/apps/videopipeline.png"
done

DESKTOP="$PREFIX/share/applications/videopipeline.desktop"
sed "s|__EXEC__|$PREFIX/bin/videopipeline|g" videopipeline.desktop > "$DESKTOP"
chmod 755 "$DESKTOP"

command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$ICONS" >/dev/null 2>&1 || true
command -v update-desktop-database >/dev/null && update-desktop-database "$PREFIX/share/applications" >/dev/null 2>&1 || true
command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 >/dev/null 2>&1 || true

echo "Zainstalowano w $PREFIX/bin. Uruchom z menu aplikacji (VideoPipeline)."
echo "Jeśli ikona nie pojawi się od razu, wyloguj się i zaloguj ponownie albo uruchom: kbuildsycoca6 --noincremental"
