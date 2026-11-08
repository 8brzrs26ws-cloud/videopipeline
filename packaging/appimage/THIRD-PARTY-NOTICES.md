# Dołączone składniki (AppImage, deb, Windows .zip)

| Składnik | Licencja | Źródło |
|---|---|---|
| VideoPipeline | GPL-3.0 | https://github.com/8brzrs26ws-cloud/videopipeline |
| FFmpeg (ffmpeg, ffprobe; budowany z `--enable-gpl` — filtr hqdn3d jest na licencji GPL — bez `--enable-nonfree`; z ffnvcodec, NVENC/NVDEC, CUDA-LLVM, dav1d; Linux: VAAPI i libvpl, Windows: libvpl, D3D11VA, DXVA2) | GPL-2.0+ | https://ffmpeg.org/download.html |
| nv-codec-headers (statycznie, tylko nagłówki) | MIT | https://github.com/FFmpeg/nv-codec-headers |
| libva, libdrm (Linux; dynamicznie — kopie w AppImage używane tylko, gdy brak ich w systemie) | MIT | https://github.com/intel/libva |
| libvpl / oneVPL dispatcher (Linux: dynamicznie, Windows: statycznie w ffmpeg.exe) | MIT | https://github.com/intel/libvpl |
| dav1d (statycznie w ffmpeg) | BSD-2-Clause | https://code.videolan.org/videolan/dav1d |
| MKVToolNix (mkvmerge) — Linux: z bibliotekami z dystrybucji Debian; Windows: oficjalna wersja przenośna | GPL-2.0 | https://mkvtoolnix.download/source.html |
| MediaInfo (CLI; Linux: z bibliotekami z dystrybucji Debian, Windows: oficjalne CLI MediaArea) | BSD-2-Clause | https://mediaarea.net/MediaInfo |
| dovi_tool | MIT | https://github.com/quietvoid/dovi_tool |
| hdr10plus_tool | MIT | https://github.com/quietvoid/hdr10plus_tool |
| Biblioteka standardowa Rusta (libstd / std-*.dll) | MIT / Apache-2.0 | https://github.com/rust-lang/rust |
| Windows: środowisko uruchomieniowe MinGW-w64 (libgcc, libstdc++, winpthreads — statycznie lub jako DLL) | GPL-3.0 z wyjątkiem GCC Runtime Library / MIT / ZPL | https://www.mingw-w64.org |

Źródła pakietów Debiana (mkvtoolnix i jego zależności) są dostępne przez `apt-get source <pakiet>`
albo na https://packages.debian.org. Wersje użyte w danym wydaniu są wypisywane na początku
logu budowania (`./build-local.sh`).

Sterowniki GPU (NVIDIA: libcuda / nvcuda.dll, NVENC, nvidia-smi; Intel: iHD, oneVPL GPU runtime)
NIE są dołączone — program używa tych z systemu.
