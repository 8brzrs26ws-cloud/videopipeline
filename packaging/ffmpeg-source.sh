# Wspólne dla buildów Linux i Windows (dołączane przez `source`): ustalanie najnowszej wersji
# ffmpeg i pobieranie źródeł z kilku źródeł, bo pojedyncze serwery bywają chwilowo niedostępne.
# Funkcje nie kończą skryptu przy błędzie sieci — zwracają pusty wynik / kod ≠ 0.

# Najnowsze wydanie ffmpeg (np. 9.0.2). Kolejno: ffmpeg.org/releases, tagi nX.Y.Z na GitHubie
# (oficjalny mirror), tagi w git.ffmpeg.org, a na końcu najnowszy tarball już pobrany do $1 (cache).
ffmpeg_latest() {
    local cache_dir="${1:-}" v=""
    v="$({ wget -qO- --timeout=20 --tries=2 https://ffmpeg.org/releases/ || true; } \
        | { grep -oE 'ffmpeg-[0-9]+\.[0-9]+(\.[0-9]+)?\.tar\.xz' || true; } \
        | sed -E 's/ffmpeg-(.*)\.tar\.xz/\1/' | sort -uV | tail -n1)"
    local repo
    for repo in https://github.com/FFmpeg/FFmpeg.git https://git.ffmpeg.org/ffmpeg.git; do
        [[ -n "$v" ]] && break
        v="$({ GIT_TERMINAL_PROMPT=0 timeout 60 git ls-remote --tags --refs "$repo" 2>/dev/null || true; } \
            | sed 's|.*refs/tags/||' | { grep -E '^n[0-9]+\.[0-9]+(\.[0-9]+)?$' || true; } \
            | sed 's/^n//' | sort -V | tail -n1)"
    done
    if [[ -z "$v" && -n "$cache_dir" ]]; then
        v="$(ls -1 "$cache_dir"/ffmpeg-*.tar.* 2>/dev/null | sed -E 's|.*/ffmpeg-(.*)\.tar\..*|\1|' | sort -V | tail -n1)"
        [[ -n "$v" ]] && echo "UWAGA: źródła wersji ffmpeg niedostępne — używam pobranej wcześniej $v" >&2
    fi
    echo "$v"
}

# Rozpakowuje źródła ffmpeg $1 do katalogu $2 jako $2/ffmpeg-$1. Archiwa trzyma w $3 (cache, wspólny
# dla Linuksa i Windows). Kolejno: archiwum z cache, ffmpeg.org (.tar.xz), archiwum tagu z GitHuba (.tar.gz).
ffmpeg_unpack() {
    local ver="$1" dest="$2" store="$3"
    local xz="$store/ffmpeg-$ver.tar.xz" gz="$store/ffmpeg-$ver.tar.gz"
    mkdir -p "$store" "$dest"
    rm -rf "$dest/ffmpeg-$ver"
    if [[ ! -s "$xz" && ! -s "$gz" ]]; then
        if wget -nv --timeout=30 --tries=3 -O "$xz.part" "https://ffmpeg.org/releases/ffmpeg-$ver.tar.xz"; then
            mv "$xz.part" "$xz"
        else
            rm -f "$xz.part"
            echo "ffmpeg.org niedostępny — pobieram tag n$ver z GitHuba" >&2
            wget -nv --timeout=30 --tries=3 -O "$gz.part" \
                "https://github.com/FFmpeg/FFmpeg/archive/refs/tags/n$ver.tar.gz" || { rm -f "$gz.part"; return 1; }
            mv "$gz.part" "$gz"
        fi
    fi
    if [[ -s "$xz" ]]; then
        tar -C "$dest" -xf "$xz"
    else
        # Archiwum tagu z GitHuba ma katalog FFmpeg-n<wersja>.
        tar -C "$dest" -xf "$gz"
        mv "$dest/FFmpeg-n$ver" "$dest/ffmpeg-$ver"
    fi
    [[ -x "$dest/ffmpeg-$ver/configure" ]]
}
