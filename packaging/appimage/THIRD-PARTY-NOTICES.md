# Dołączone składniki (AppImage)

| Składnik | Licencja | Źródło |
|---|---|---|
| VideoPipeline | GPL-3.0 | https://github.com/8brzrs26ws-cloud/videopipeline |
| FFmpeg (ffmpeg, ffprobe; budowany bez `--enable-gpl`/`--enable-nonfree`, z ffnvcodec, NVENC/NVDEC, CUDA-LLVM, dav1d) | LGPL-2.1+ | https://ffmpeg.org/download.html |
| nv-codec-headers (statycznie, tylko nagłówki) | MIT | https://github.com/FFmpeg/nv-codec-headers |
| dav1d (statycznie w ffmpeg) | BSD-2-Clause | https://code.videolan.org/videolan/dav1d |
| MKVToolNix (mkvmerge) wraz z bibliotekami z dystrybucji Ubuntu | GPL-2.0 | https://mkvtoolnix.download/source.html |
| dovi_tool | MIT | https://github.com/quietvoid/dovi_tool |
| hdr10plus_tool | MIT | https://github.com/quietvoid/hdr10plus_tool |
| Biblioteka standardowa Rusta (libstd) | MIT / Apache-2.0 | https://github.com/rust-lang/rust |

Źródła pakietów Ubuntu (mkvtoolnix i jego zależności) są dostępne przez `apt-get source <pakiet>`
albo na https://packages.ubuntu.com. Wszystkie wersje użyte w danym wydaniu wynikają z logu
workflow „Release” w GitHub Actions.

Sterowniki NVIDIA (libcuda, libnvidia-encode, nvidia-smi) NIE są dołączone — program używa
tych z systemu hosta (wymagany sterownik NVIDIA z obsługą NVENC).
