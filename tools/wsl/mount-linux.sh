#!/usr/bin/env bash
# Strona Linuksa dla mount-linux.ps1 (uruchamiana w WSL jako root):
#   detect                  — PARTUUID|FSTYPE|LABEL partycji dysków podłączonych przez "wsl --mount --bare"
#   links nazwa=ścieżka ... — dowiązania /mnt/linux/<nazwa> do partycji zamontowanych przez WSL w /mnt/wsl,
#                             katalogi domowe i znalezione repozytoria videopipeline
#   unlink                  — usuwa dowiązania z /mnt/linux
# Montuje sam WSL (wsl --mount --partition): /mnt/wsl jest wspólne dla wszystkich sesji i dystrybucji,
# a mount wykonany tutaj byłby widoczny tylko w sesji administratora.
set -uo pipefail
MODE="${1:-}"; shift || true
BASE=/mnt/linux
VP_UID="${VP_UID:-1000}"
[[ $EUID -eq 0 ]] || { echo "Uruchom jako root (robi to mount-linux.ps1)." >&2; exit 1; }

case "$MODE" in
detect)
    # Dyski wirtualne WSL nie mają tablicy partycji — bierzemy tylko partycje z PARTUUID.
    lsblk -Pno PARTUUID,TYPE,FSTYPE,LABEL | while read -r line; do
        eval "$line"
        [[ "$TYPE" = part && -n "$PARTUUID" ]] && echo "$PARTUUID|$FSTYPE|$LABEL"
    done
    ;;
unlink)
    for l in "$BASE"/*; do
        [[ -L "$l" ]] && rm -f "$l" && echo "usunięto $l"
    done
    # Puste katalogi po starszej wersji skryptu (montowanie w /mnt/linux).
    for d in "$BASE"/*; do
        [[ -d "$d" && ! -L "$d" ]] && ! mountpoint -q "$d" && rmdir "$d" 2>/dev/null
    done
    ;;
links)
    mkdir -p "$BASE"
    targets=()
    for pair in "$@"; do
        name="${pair%%=*}"; target="${pair#*=}"
        name="${name//[^A-Za-z0-9._-]/_}"
        link="$BASE/$name"
        # Pusty katalog po starszej wersji skryptu → zastąp dowiązaniem.
        [[ -d "$link" && ! -L "$link" ]] && ! mountpoint -q "$link" && rmdir "$link" 2>/dev/null
        if [[ -e "$link" && ! -L "$link" ]]; then
            echo "  $link istnieje i nie jest dowiązaniem — pomijam (partycja: $target)"
        else
            ln -sfn "$target" "$link"
            echo "  $link → $target"
        fi
        targets+=("$target")
    done

    echo
    echo "Katalogi domowe:"
    for mp in "${targets[@]}"; do
        for h in "$mp"/@home/* "$mp"/home/*; do
            [[ -d "$h" ]] || continue
            owner="$(stat -c %u "$h")"
            note=""
            [[ "$owner" != "$VP_UID" ]] && note="  (UWAGA: właściciel UID $owner, twój użytkownik WSL ma UID $VP_UID — zapis może wymagać sudo)"
            echo "  $h$note"
        done
    done

    echo
    echo "Repozytoria videopipeline:"
    found=0
    for mp in "${targets[@]}"; do
        while read -r g; do
            echo "  ${g%/.git}"; found=1
        done < <(find "$mp"/@home "$mp"/home -maxdepth 7 -type d -name .git -path '*videopipeline*' 2>/dev/null)
    done
    [[ $found = 1 ]] || echo "  (nie znaleziono w katalogach domowych)"
    echo
    echo "Budowanie z tego repozytorium:  cd <ścieżka powyżej> && ./build-local.sh windows"
    echo "Przed uruchomieniem Linuksa:     tools\\wsl\\mount-linux.cmd -Unmount (albo wsl --shutdown)"
    ;;
*)
    echo "Użycie: $0 detect | links nazwa=ścieżka... | unlink" >&2; exit 1 ;;
esac
