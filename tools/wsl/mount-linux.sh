#!/usr/bin/env bash
# Strona Linuksa dla mount-linux.ps1 (uruchamiana w WSL jako root):
#   mount   — montuje partycje dysków podłączonych przez "wsl --mount --bare" w /mnt/linux/<nazwa>
#   unmount — odmontowuje je (i zamyka odszyfrowane LUKS)
# Dyski wirtualne WSL (system, swap, dystrybucja) nie mają tablicy partycji — bierzemy tylko partycje.
set -uo pipefail
MODE="${1:-mount}"
BASE=/mnt/linux
VP_UID="${VP_UID:-1000}"
[[ $EUID -eq 0 ]] || { echo "Uruchom jako root (robi to mount-linux.ps1)." >&2; exit 1; }

if [[ "$MODE" = unmount ]]; then
    for m in $(findmnt -rn -o TARGET | grep "^$BASE/" | sort -r); do
        umount "$m" && echo "odmontowano $m"
    done
    for d in /dev/mapper/vp-*; do
        [[ -e "$d" ]] && cryptsetup close "$(basename "$d")" && echo "zamknięto $(basename "$d")"
    done
    rmdir "$BASE"/* 2>/dev/null
    exit 0
fi

mkdir -p "$BASE"
mounted=()

mount_one() {   # $1 = urządzenie, $2 = system plików, $3 = etykieta
    local dev="$1" fs="$2" label="$3" name mp opts
    if findmnt -rn -S "$dev" >/dev/null; then
        echo "  $dev już zamontowany w $(findmnt -rn -S "$dev" -o TARGET | head -n1)"
        mounted+=("$(findmnt -rn -S "$dev" -o TARGET | head -n1)")
        return
    fi
    if ! grep -qw "$fs" /proc/filesystems && ! modprobe "$fs" 2>/dev/null; then
        echo "  $dev ($fs): jądro WSL nie obsługuje $fs — spróbuj w PowerShellu: wsl --update"
        return
    fi
    label="$(printf '%b' "$label")"   # lsblk -r zapisuje spacje jako \x20
    name="${label:-$(basename "$dev")}"
    name="${name//[^A-Za-z0-9._-]/_}"
    mp="$BASE/$name"
    [[ -e "$mp" ]] && mp="$BASE/$name-$(basename "$dev")"
    mkdir -p "$mp"
    opts="noatime"
    # btrfs: główny poziom (subvolid=5) — widać wszystkie podwolumeny (@, @home…).
    [[ "$fs" = btrfs ]] && opts="noatime,subvolid=5"
    if mount -t "$fs" -o "$opts" "$dev" "$mp"; then
        echo "  $dev ($fs${label:+, „$label”}) → $mp"
        mounted+=("$mp")
    else
        rmdir "$mp" 2>/dev/null
        echo "  $dev ($fs): montowanie nie powiodło się (Linux zamknięty normalnie, bez hibernacji?)"
    fi
}

echo "Partycje Linuksa w WSL:"
while read -r dev type fs label; do
    [[ "$type" = part ]] || continue
    case "$fs" in
        ext4|ext3|ext2|btrfs|xfs|f2fs)
            mount_one "$dev" "$fs" "$label" ;;
        crypto_LUKS)
            command -v cryptsetup >/dev/null || { echo "  $dev: szyfrowana (LUKS) — zainstaluj: sudo apt install cryptsetup"; continue; }
            map="vp-$(basename "$dev")"
            if [[ ! -e "/dev/mapper/$map" ]]; then
                echo "  $dev: szyfrowana (LUKS) — podaj hasło (Enter z pustym = pomiń)"
                cryptsetup open "$dev" "$map" </dev/tty || continue
            fi
            read -r ifs ilabel < <(lsblk -rno FSTYPE,LABEL "/dev/mapper/$map" | head -n1)
            [[ -n "${ifs:-}" ]] && mount_one "/dev/mapper/$map" "$ifs" "${ilabel:-$map}" ;;
        swap|"") ;;
        *) echo "  $dev: system plików $fs — pomijam" ;;
    esac
done < <(lsblk -rpno NAME,TYPE,FSTYPE,LABEL)

[[ ${#mounted[@]} -gt 0 ]] || { echo "Nic nie zamontowano."; exit 1; }

echo
echo "Katalogi domowe:"
for mp in "${mounted[@]}"; do
    for h in "$mp"/@home/* "$mp"/home/*; do
        [[ -d "$h" ]] || continue
        owner="$(stat -c %u "$h")"
        note=""; [[ "$owner" != "$VP_UID" ]] && note="  (UWAGA: właściciel UID $owner, a twój użytkownik WSL ma UID $VP_UID — zapis może wymagać sudo)"
        echo "  $h$note"
    done
done

echo
echo "Repozytoria videopipeline:"
found=0
for mp in "${mounted[@]}"; do
    while read -r g; do
        echo "  ${g%/.git}"; found=1
    done < <(find "$mp"/@home "$mp"/home -maxdepth 7 -type d -name .git -path '*videopipeline*' 2>/dev/null)
done
[[ $found = 1 ]] || echo "  (nie znaleziono w katalogach domowych)"
echo
echo "Budowanie z tego repozytorium:  cd <ścieżka powyżej> && ./build-local.sh windows"
echo "Przed uruchomieniem Linuksa:     tools\\wsl\\mount-linux.cmd -Unmount (albo wsl --shutdown)"
