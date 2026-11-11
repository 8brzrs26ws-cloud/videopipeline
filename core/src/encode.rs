use crate::{
    analysis::Analysis,
    config::*,
    model::{read_progress, Panel, State},
    probe,
    proc::{self, PIDS},
};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, ExitStatus, Stdio},
    thread,
    time::Duration,
};

/// Dolby Vision / HDR10+ w locie: koder → wątek z RPU i SEI HDR10+ → ffmpeg składający plik wynikowy.
struct Live {
    mux: Child,
    thread: thread::JoinHandle<anyhow::Result<crate::inject::Stats>>,
    mux_log: PathBuf,
}

struct Running {
    child: Child,
    live: Option<Live>,
    name: String,
    /// Plik zapisywany przez ffmpeg: tymczasowe wideo albo <wynik>.part (tryb bezpośredni).
    output: PathBuf,
    /// Tryb bezpośredni: docelowa nazwa pliku wynikowego (po sukcesie .part → wynik).
    final_out: Option<PathBuf>,
    log: PathBuf,
    pf: PathBuf,
}

fn sv(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Wyjście ffmpeg: mapowanie strumieni (przed opcjami kodera) i końcówka z plikiem.
struct OutSpec {
    streams: Vec<String>,
    tail: Vec<String>,
}

/// Opcje kodera wideo tylko dla strumienia wideo (`-preset` → `-preset:v`) — przy kodowaniu wprost
/// do kontenera w tym samym poleceniu są też strumienie audio (kopia / E-AC-3).
fn scope_v(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut opt = true; // na przemian: opcja, wartość
    for a in args {
        if opt && a.starts_with('-') && !a.contains(':') {
            out.push(format!("{a}:v"));
        } else {
            out.push(a);
        }
        opt = !opt;
    }
    out
}

/// Tryb z plikiem tymczasowym: samo wideo do surowego .hevc / .mkv (AV1), dalej DV/HDR10+ i mux.
fn temp_out(path: &Path) -> OutSpec {
    OutSpec { streams: sv(&["-an", "-sn", "-map", "0:v:0"]), tail: vec![path.to_string_lossy().into()] }
}

/// Tryb bezpośredni: wideo + audio (kopia lub E-AC-3 w locie) + napisy od razu w pliku wynikowym.
fn direct_out(opts: &Opts, a: &Analysis, p: &Paths, part: &Path) -> OutSpec {
    let info = probe::stream_codecs(&a.input_file);
    let mut streams = sv(&["-map", "0:v:0"]);
    let (src, _) = crate::mux::source_streams(opts, a, p.container, 0, true, &info);
    streams.extend(src);
    let mut tail = sv(&["-map_metadata", "0", "-map_chapters", "0"]);
    match p.container {
        Container::Mp4 => {
            tail.extend(sv(&["-movflags", "+faststart"]));
            if opts.codec == Codec::Hevc {
                tail.extend(sv(&["-tag:v", "hvc1"])); // zgodność z odtwarzaczami (Apple, przeglądarki)
            }
            tail.extend(sv(&["-f", "mp4"]));
        }
        Container::Mkv => tail.extend(sv(&["-f", "matroska"])),
    }
    tail.extend(sv(&["-y"]));
    tail.push(part.to_string_lossy().into());
    OutSpec { streams, tail }
}

/// Tryb w locie, koder: samo wideo jako surowe HEVC na stdout (czyta je wstrzykiwacz).
fn live_encoder_out() -> OutSpec {
    OutSpec { streams: sv(&["-an", "-sn", "-map", "0:v:0"]), tail: sv(&["-f", "hevc", "pipe:1"]) }
}

/// Tryb w locie, drugi ffmpeg: wideo z metadanymi (Matroska z potoku) + audio i napisy źródła.
fn live_mux_args(opts: &Opts, a: &Analysis, p: &Paths, part: &Path) -> Vec<String> {
    let info = probe::stream_codecs(&a.input_file);
    let mut v = sv(&["-hide_banner", "-nostats", "-loglevel", "warning", "-f", "matroska", "-i", "pipe:0", "-i"]);
    v.push(a.input_file.to_string_lossy().into());
    v.extend(sv(&["-map", "0:v:0", "-c:v", "copy"]));
    let (src, _) = crate::mux::source_streams(opts, a, p.container, 1, true, &info);
    v.extend(src);
    v.extend(sv(&["-map_metadata", "1", "-map_chapters", "1"]));
    match p.container {
        // hvc1 dla odtwarzaczy Apple / przeglądarek; dvvC w MP4 wymaga -strict unofficial.
        Container::Mp4 => v.extend(sv(&["-tag:v", "hvc1", "-strict", "unofficial", "-movflags", "+faststart", "-f", "mp4"])),
        Container::Mkv => v.extend(sv(&["-f", "matroska"])),
    }
    v.extend(sv(&["-y"]));
    v.push(part.to_string_lossy().into());
    v
}

/// "24000/1001" → (24000, 1001)
fn parse_fps(s: &str) -> Option<(u64, u64)> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d) = (n.trim().parse::<u64>().ok()?, d.trim().parse::<u64>().ok()?);
    (n > 0 && d > 0).then_some((n, d))
}

/// Mapowanie tonów HDR (PQ/HLG, BT.2020) → SDR BT.709 8-bit dla AVC. Krzywa Hable bez desaturacji.
/// CPU: zscale (libzimg) liniowo → tonemap → BT.709.
pub(crate) const TONEMAP_CPU: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p";
/// GPU przez OpenCL (NVIDIA / Intel): klatki z pamięci → karta → z powrotem (koder NVENC bierze z pamięci).
pub(crate) const TONEMAP_OPENCL: &str = "format=p010le,hwupload,tonemap_opencl=tonemap=hable:desat=0:\
t=bt709:m=bt709:p=bt709:r=tv:format=nv12,hwdownload,format=nv12";
/// Intel QSV: opcje vpp_qsv (sprzętowe mapowanie tonów z metadanych HDR źródła).
pub(crate) const TONEMAP_QSV: &str = "tonemap=1:out_color_matrix=bt709:out_color_primaries=bt709:out_color_transfer=bt709";
/// Intel VAAPI: filtr po hwupload / dekodowaniu VAAPI.
pub(crate) const TONEMAP_VAAPI: &str = "tonemap_vaapi=format=nv12:t=bt709:m=bt709:p=bt709";

/// Znaczniki koloru wyniku SDR: jak źródło, a po mapowaniu tonów — BT.709.
fn sdr_tags(c: &crate::probe::ColorProfile, tonemapped: bool) -> Vec<String> {
    if tonemapped {
        sv(&["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709", "-color_range", "tv"])
    } else {
        sv(&[
            "-color_primaries", &c.sdr_primaries, "-color_trc", &c.sdr_trc,
            "-colorspace", &c.sdr_space, "-color_range", &c.sdr_range,
        ])
    }
}

fn build_args_nvenc(
    opts: &Opts,
    rt: &Runtime,
    a: &Analysis,
    out: &OutSpec,
    pf: &Path,
) -> (Vec<String>, String) {
    let c = &a.color;
    let mut hw: Vec<String> = vec![];
    let mut fmt: Vec<String> = vec![];
    let mut filt: Vec<String> = vec![];
    let note;
    // Kadrowanie i skalowanie na CPU: NVENC i tak dostaje klatki z pamięci (dekodowanie CUDA bez
    // -hwaccel_output_format). Skalowanie przed odszumianiem / mapowaniem tonów — mniej pikseli dalej.
    let scale = scale_target(opts, a);
    let pre: Vec<String> = a.crop.iter().cloned().chain(scale.as_ref().map(|s| s.cpu())).collect();

    // AVC jest zawsze 8-bit SDR (źródła HDR odrzuca analiza; 10-bit SDR jest przeliczane do 8 bitów).
    let avc = opts.codec == Codec::Avc;
    if c.is_sdr_8bit || avc {
        match opts.codec {
            Codec::Hevc => fmt.extend(sv(&["-profile:v", "main"])),
            Codec::Avc => fmt.extend(sv(&["-profile:v", "high"])),
            Codec::Av1 => {}
        }
        // AVC ze źródła HDR: mapowanie tonów do SDR BT.709 (OpenCL na karcie albo CPU).
        let tonemap = avc && c.is_hdr();
        fmt.extend(sv(&["-pix_fmt", "yuv420p"]));
        fmt.extend(sdr_tags(c, tonemap));

        if opts.denoise && c.is_sdr_8bit && rt.denoise != Denoise::Off {
            let mut chain: Vec<String> = pre.clone();
            if rt.denoise == Denoise::Cuda {
                chain.push(format!(
                    "format=yuv420p,hwupload_cuda,{DENOISE_CUDA_FILTER},hwdownload,format=yuv420p"
                ));
                hw = sv(&["-init_hw_device", "cuda=denoise:0", "-filter_hw_device", "denoise"]);
                note = "SDR 8-bit + CUDA denoise";
            } else {
                chain.push(DENOISE_CPU_FILTER.into());
                note = "SDR 8-bit + CPU denoise";
            }
            filt = vec!["-vf".into(), chain.join(",")];
        } else if tonemap {
            let mut chain: Vec<String> = pre.clone();
            if rt.tonemap == Tonemap::OpenCl {
                hw = sv(&["-init_hw_device", "opencl=ocl", "-filter_hw_device", "ocl"]);
                chain.push(TONEMAP_OPENCL.into());
                note = "HDR -> SDR AVC 8-bit (tone mapping OpenCL)";
            } else {
                chain.push(TONEMAP_CPU.into());
                note = "HDR -> SDR AVC 8-bit (tone mapping CPU)";
            }
            filt = vec!["-vf".into(), chain.join(",")];
        } else {
            if !pre.is_empty() {
                filt = vec!["-vf".into(), pre.join(",")];
            }
            note = match opts.codec {
                Codec::Av1 => "SDR 8-bit -> AV1 10-bit (highbitdepth)",
                Codec::Avc if !c.is_sdr_8bit => "SDR 10-bit -> AVC 8-bit (High)",
                Codec::Avc => "SDR 8-bit -> AVC High",
                Codec::Hevc => "SDR 8-bit",
            };
        }
    } else {
        if opts.codec == Codec::Hevc {
            fmt.extend(sv(&["-profile:v", "main10"]));
        }
        fmt.extend(sv(&["-pix_fmt", "p010le", "-color_primaries", "bt2020"]));
        fmt.extend(sv(&["-color_trc", "smpte2084", "-colorspace", "bt2020nc"]));
        if !pre.is_empty() {
            filt = vec!["-vf".into(), pre.join(",")];
        }
        note = if opts.denoise {
            "HDR — denoise pominięty (tylko SDR 8-bit)"
        } else {
            "HDR 10-bit"
        };
    }

    let cq = opts.cq().to_string();
    let enc: Vec<String> = match opts.codec {
        Codec::Av1 => sv(&[
            "-c:v", "av1_nvenc", "-preset", "p7", "-tune", &rt.av1_tune, "-rc", "vbr",
            "-cq", &cq, "-b:v", "0", "-spatial-aq", "1", "-temporal-aq", "1", "-bf", "4",
            "-b_ref_mode", "middle", "-multipass", "fullres", "-rc-lookahead", "32",
            "-highbitdepth", "1",
        ]),
        Codec::Hevc => sv(&[
            "-c:v", "hevc_nvenc", "-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq",
            &cq, "-b:v", "0", "-spatial-aq", "1", "-temporal-aq", "1", "-b_ref_mode",
            "middle", "-multipass", "2", "-rc-lookahead", "32",
        ]),
        // AVC: te same priorytety jakości co HEVC (p7/hq, VBR CQ, AQ, B-klatki referencyjne,
        // pełne multipass, lookahead) + CABAC.
        Codec::Avc => sv(&[
            "-c:v", "h264_nvenc", "-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq", &cq,
            "-b:v", "0", "-spatial-aq", "1", "-temporal-aq", "1", "-bf", "3", "-b_ref_mode",
            "middle", "-multipass", "fullres", "-rc-lookahead", "32", "-coder", "cabac",
        ]),
    };

    let mut v = sv(&["-hide_banner", "-nostats", "-progress"]);
    v.push(pf.to_string_lossy().into());
    v.extend(hw);
    // Oba równoległe enkodery dekodują przez CUDA/NVDEC.
    v.extend(sv(&["-hwaccel", "cuda"]));
    v.push("-i".into());
    v.push(a.input_file.to_string_lossy().into());
    v.extend(filt);
    v.extend(out.streams.iter().cloned());
    v.extend(scope_v(enc));
    v.extend(scope_v(fmt));
    v.extend(out.tail.iter().cloned());
    let note = match &scale {
        Some(s) => format!("{note} · skalowanie {}x{} → {}x{}", s.src.0, s.src.1, s.dst.0, s.dst.1),
        None => note.to_string(),
    };
    (v, note)
}


/// Skalowanie do 1080p: (szerokość, wysokość) po kadrowaniu i docelowe — tylko gdy obraz jest
/// większy niż 1920×1080. Proporcje zachowane, wymiary parzyste (4:2:0).
pub(crate) struct Scale {
    pub src: (u32, u32),
    pub dst: (u32, u32),
}

impl Scale {
    /// Filtr CPU (swscale, Lanczos) — po crop, przed odszumianiem i mapowaniem tonów.
    fn cpu(&self) -> String {
        format!("scale={}:{}:flags=lanczos", self.dst.0, self.dst.1)
    }
    /// Współczynniki dla przesunięć obszaru aktywnego (L5) w RPU Dolby Vision.
    pub fn factors(&self) -> (f64, f64) {
        (self.dst.0 as f64 / self.src.0 as f64, self.dst.1 as f64 / self.src.1 as f64)
    }
}

fn fit_1080(w: u32, h: u32) -> Option<(u32, u32)> {
    if w <= 1920 && h <= 1080 {
        return None;
    }
    let f = (1920.0 / w as f64).min(1080.0 / h as f64);
    let even = |x: f64| (((x / 2.0).round() as u32) * 2).max(2);
    Some((even(w as f64 * f), even(h as f64 * f)))
}

pub(crate) fn scale_target(opts: &Opts, a: &Analysis) -> Option<Scale> {
    if !opts.scale_1080 {
        return None;
    }
    let src = match a.crop.as_deref().and_then(parse_crop) {
        Some((w, h, _, _)) => (w, h),
        None => probe::video_size(&a.input_file)?,
    };
    fit_1080(src.0, src.1).map(|dst| Scale { src, dst })
}

/// "crop=W:H:X:Y" → (W, H, X, Y)
fn parse_crop(s: &str) -> Option<(u32, u32, u32, u32)> {
    let v: Vec<u32> = s.strip_prefix("crop=")?.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
}

/// Dekoder QSV dla kodeka źródła (nazwa z ffprobe), jeśli ffmpeg go ma.
fn qsv_decoder(rt: &Runtime, src_codec: Option<&str>) -> Option<String> {
    let name = match src_codec? {
        "h264" => "h264_qsv",
        "hevc" => "hevc_qsv",
        "av1" => "av1_qsv",
        "vp9" => "vp9_qsv",
        "vp8" => "vp8_qsv",
        "mpeg2video" => "mpeg2_qsv",
        "vc1" => "vc1_qsv",
        "mjpeg" => "mjpeg_qsv",
        _ => return None,
    };
    rt.qsv_decoders.iter().any(|d| d == name).then(|| name.to_string())
}

/// Intel: QSV (dekodowanie QSV + vpp_qsv) albo VAAPI (dekodowanie VAAPI + filtry VAAPI).
/// Zwraca (argumenty, opis, dekoder).
fn build_args_intel(
    opts: &Opts,
    rt: &Runtime,
    a: &Analysis,
    src_codec: Option<&str>,
    out: &OutSpec,
    pf: &Path,
) -> (Vec<String>, String, String) {
    let c = &a.color;
    let dev = rt.intel_dev.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    // AV1 zawsze 10-bit (jak -highbitdepth w NVENC); HEVC 10-bit tylko dla źródeł HDR/10-bit.
    let ten_bit = (!c.is_sdr_8bit || opts.codec == Codec::Av1) && opts.codec != Codec::Avc;
    let crop = a.crop.as_deref().and_then(parse_crop);
    let scale = scale_target(opts, a);
    let denoise = opts.denoise && c.is_sdr_8bit && rt.denoise != Denoise::Off;
    // AVC ze źródła HDR: mapowanie tonów (sprzętowe Intel albo CPU — wtedy klatki przez pamięć).
    let tonemap = opts.codec == Codec::Avc && c.is_hdr();
    let cpu_tonemap = tonemap && !matches!(rt.tonemap, Tonemap::Qsv | Tonemap::Vaapi);
    let cq = opts.cq() as u32;

    let mut v = sv(&["-hide_banner", "-nostats", "-progress"]);
    v.push(pf.to_string_lossy().into());

    let mut notes: Vec<String> = vec![match (c.is_sdr_8bit, opts.codec) {
        (true, _) => "SDR 8-bit".into(),
        (false, Codec::Avc) if tonemap => format!("HDR → SDR AVC 8-bit (tone mapping {})", if cpu_tonemap { "CPU" } else { "GPU Intel" }),
        (false, Codec::Avc) => "SDR 10-bit → AVC 8-bit".into(),
        (false, _) => "HDR 10-bit".into(),
    }];
    if opts.codec == Codec::Av1 && c.is_sdr_8bit {
        notes.push("→ AV1 10-bit".into());
    }
    if let Some(s) = &scale {
        notes.push(format!("skalowanie {}x{} → {}x{}", s.src.0, s.src.1, s.dst.0, s.dst.1));
    }
    let decoder;
    let enc: Vec<String>;

    match opts.engine {
        Engine::Qsv => {
            // Windows, tryb "sysmem": bez urządzenia i pul ramek ffmpeg (patrz hw::WIN_QSV_DEVICES) —
            // dekoder QSV oddaje klatki do pamięci, filtry CPU, koder QSV sam wgrywa je na kartę.
            let sysmem = crate::hw::qsv_is_sysmem(&dev);
            v.extend(crate::hw::qsv_init_args(&dev));
            if !sysmem {
                v.extend(sv(&["-filter_hw_device", "qs"]));
            }
            // Filtry na CPU (Windows "sysmem" albo mapowanie tonów na CPU): klatki z dekodera do pamięci.
            let cpu_filters = sysmem || cpu_tonemap;
            match qsv_decoder(rt, src_codec) {
                Some(d) if cpu_filters => {
                    v.extend(sv(&["-c:v", &d]));
                    decoder = format!("QSV ({d}, przez pamięć)");
                }
                Some(d) => {
                    v.extend(sv(&["-hwaccel", "qsv", "-hwaccel_device", "qs", "-hwaccel_output_format", "qsv"]));
                    v.extend(sv(&["-c:v", &d]));
                    decoder = format!("QSV ({d})");
                }
                None => decoder = format!("CPU (brak dekodera QSV dla {})", src_codec.unwrap_or("?")),
            }
            v.push("-i".into());
            v.push(a.input_file.to_string_lossy().into());

            if cpu_filters {
                let mut chain: Vec<String> = vec![];
                if let Some(cr) = &a.crop {
                    chain.push(cr.clone());
                }
                if let Some(s) = &scale {
                    chain.push(s.cpu());
                }
                if cpu_tonemap {
                    chain.push(TONEMAP_CPU.into());
                }
                if denoise {
                    chain.push(DENOISE_CPU_FILTER.into());
                    notes.push("denoise CPU".into());
                }
                chain.push(format!("format={}", if ten_bit { "p010le" } else { "nv12" }));
                v.extend(sv(&["-vf", &chain.join(",")]));
            } else {
                // vpp_qsv przyjmuje klatki z karty i z pamięci (dekodowanie programowe) — jeden filtr na wszystko.
                let mut vpp: Vec<String> = vec![];
                if let Some((w, h, x, y)) = crop {
                    vpp.push(format!("cw={w}:ch={h}:cx={x}:cy={y}"));
                }
                if let Some(s) = &scale {
                    vpp.push(format!("w={}:h={}", s.dst.0, s.dst.1));
                }
                if denoise && rt.denoise == Denoise::Intel {
                    vpp.push(format!("denoise={DENOISE_QSV_LEVEL}"));
                    notes.push("denoise vpp_qsv".into());
                }
                if tonemap {
                    vpp.push(TONEMAP_QSV.into());
                }
                vpp.push(format!("format={}", if ten_bit { "p010" } else { "nv12" }));
                v.extend(sv(&["-vf", &format!("vpp_qsv={}", vpp.join(":"))]));
            }

            enc = match opts.codec {
                Codec::Hevc => sv(&[
                    "-c:v", "hevc_qsv", "-preset", opts.qsv_preset, "-global_quality", &cq.to_string(),
                    "-profile:v", if ten_bit { "main10" } else { "main" },
                ]),
                Codec::Av1 => sv(&[
                    "-c:v", "av1_qsv", "-preset", opts.qsv_preset, "-global_quality", &cq.to_string(),
                    "-profile:v", "main",
                ]),
                Codec::Avc => sv(&[
                    "-c:v", "h264_qsv", "-preset", opts.qsv_preset, "-global_quality", &cq.to_string(),
                    "-profile:v", "high",
                ]),
            };
            notes.push(format!("ICQ {cq}"));
        }
        _ => {
            // VAAPI. Crop i odszumianie na CPU wymagają klatek w pamięci: dekodowanie nadal sprzętowe,
            // ale bez -hwaccel_output_format (ffmpeg sam pobiera klatki), potem hwupload.
            let sw_fmt = if ten_bit { "p010le" } else { "nv12" };
            let cpu_denoise = denoise && rt.denoise == Denoise::Cpu;
            let sysmem = crop.is_some() || cpu_denoise || cpu_tonemap;
            v.extend(sv(&["-init_hw_device", &format!("vaapi=va:{dev}"), "-filter_hw_device", "va"]));
            v.extend(sv(&["-hwaccel", "vaapi", "-hwaccel_device", "va"]));
            if !sysmem {
                v.extend(sv(&["-hwaccel_output_format", "vaapi"]));
            }
            decoder = "VAAPI (zapasowo CPU)".into();
            v.push("-i".into());
            v.push(a.input_file.to_string_lossy().into());

            let mut chain: Vec<String> = vec![];
            if sysmem {
                if let Some(cr) = &a.crop {
                    chain.push(cr.clone());
                }
                if let Some(s) = &scale {
                    chain.push(s.cpu());
                }
                if cpu_denoise {
                    chain.push(DENOISE_CPU_FILTER.into());
                    notes.push("denoise CPU".into());
                }
                if cpu_tonemap {
                    chain.push(TONEMAP_CPU.into());
                }
                // Po sprzętowym mapowaniu tonów klatki są jeszcze 10-bit — tonemap_vaapi zamieni je na nv12.
                let up_fmt = if tonemap && !cpu_tonemap { "p010le" } else { sw_fmt };
                chain.push(format!("format={up_fmt}"));
                chain.push("hwupload".into());
                if tonemap && !cpu_tonemap {
                    chain.push(TONEMAP_VAAPI.into());
                }
            } else {
                // Klatki z dekodera VAAPI przechodzą bez kopiowania; po dekodowaniu programowym — hwupload.
                chain.push("format=nv12|p010le|vaapi".into());
                chain.push("hwupload".into());
                let size = scale.as_ref().map(|s| format!("w={}:h={}:", s.dst.0, s.dst.1)).unwrap_or_default();
                if tonemap {
                    chain.push(TONEMAP_VAAPI.into());
                    if !size.is_empty() {
                        chain.push(format!("scale_vaapi={size}format=nv12"));
                    }
                } else {
                    chain.push(format!("scale_vaapi={size}format={}", if ten_bit { "p010" } else { "nv12" }));
                }
            }
            if denoise && rt.denoise == Denoise::Intel {
                chain.push(format!("denoise_vaapi=denoise={DENOISE_VAAPI_LEVEL}"));
                notes.push("denoise_vaapi".into());
            }
            v.extend(sv(&["-vf", &chain.join(",")]));

            enc = match opts.codec {
                Codec::Hevc => sv(&[
                    "-c:v", "hevc_vaapi", "-rc_mode", "CQP", "-qp", &cq.to_string(),
                    "-profile:v", if ten_bit { "main10" } else { "main" },
                ]),
                // AV1 VAAPI: base q index 0–255 — CQ przeliczone ×5 (25 → 125).
                Codec::Av1 => sv(&[
                    "-c:v", "av1_vaapi", "-rc_mode", "CQP", "-qp", &(cq * 5).min(255).to_string(),
                    "-profile:v", "main",
                ]),
                Codec::Avc => sv(&[
                    "-c:v", "h264_vaapi", "-rc_mode", "CQP", "-qp", &cq.to_string(), "-profile:v", "high",
                ]),
            };
            notes.push(format!("CQP {cq}"));
        }
    }

    v.extend(out.streams.iter().cloned());
    v.extend(scope_v(enc));
    let color = if c.is_sdr_8bit || opts.codec == Codec::Avc {
        sdr_tags(c, tonemap)
    } else {
        sv(&["-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc"])
    };
    v.extend(scope_v(color));
    v.extend(out.tail.iter().cloned());
    (v, notes.join(" · "), decoder)
}

/// Sukces = niepusty plik + progress=end + właściwy kodek.
fn result_valid(opts: &Opts, out: &Path, pf: &Path) -> bool {
    if !fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false) {
        return false;
    }
    if !read_progress(pf).ended {
        return false;
    }
    probe::video_codec(out).as_deref() == Some(opts.codec.name())
}

/// HEVC: stara semantyka V9 (istnienie pliku). AV1: kodek + długość ≈ źródło.
fn existing_temp_valid(opts: &Opts, out: &Path, input: &Path) -> bool {
    if !fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false) {
        return false;
    }
    if opts.codec != Codec::Av1 {
        return true;
    }
    if probe::video_codec(out).as_deref() != Some("av1") {
        return false;
    }
    match (probe::container_duration(input), probe::container_duration(out)) {
        (Some(s), Some(o)) => o + (s * 0.005).max(2.0) >= s,
        _ => true,
    }
}

fn start(
    opts: &Opts,
    rt: &Runtime,
    a: &Analysis,
    slot: usize,
    panel: &Panel,
) -> std::io::Result<Running> {
    let p = opts.paths_for(&a.input_file);
    let key = sanitize(&a.filename);
    let pf = rt.progress_dir.join(format!("{key}.ffprogress"));
    let log = rt.progress_dir.join(format!("{key}.log"));
    let _ = fs::remove_file(&pf);
    File::create(&pf)?;

    let dur_us = probe::container_duration(&a.input_file).map(|d| d * 1e6);
    panel.set_progress(&a.filename, pf.clone(), dur_us);

    if opts.codec == Codec::Avc && a.color.is_hdr() && rt.tonemap == Tonemap::None {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "źródło HDR, a ffmpeg nie ma filtrów mapowania tonów (zscale/tonemap) — AVC niemożliwe; wybierz HEVC albo AV1",
        ));
    }
    let direct = opts.direct_encode(a.has_dovi, a.has_hdr10plus);
    // DV/HDR10+ w locie (domyślnie): bez pliku tymczasowego wideo i bez dovi_tool / mkvmerge.
    let live = !direct && !opts.legacy_inject;
    let fps = if live {
        let fps = probe::video_fps(&a.input_file).as_deref().and_then(parse_fps);
        if fps.is_none() {
            return Err(std::io::Error::other("nie można odczytać liczby klatek na sekundę źródła (ffprobe)"));
        }
        fps
    } else {
        None
    };
    let part = PathBuf::from(format!("{}.part", p.output.display()));
    let (write_to, out) = if direct {
        let _ = fs::remove_file(&part);
        (part.clone(), direct_out(opts, a, &p, &part))
    } else if live {
        let _ = fs::remove_file(&part);
        (part.clone(), live_encoder_out())
    } else {
        (p.temp_video.clone(), temp_out(&p.temp_video))
    };
    let (args, note, decoder) = if opts.engine == Engine::Nvenc {
        let (args, note) = build_args_nvenc(opts, rt, a, &out, &pf);
        (args, note, "CUDA/NVDEC".to_string())
    } else {
        let src = probe::video_codec(&a.input_file);
        build_args_intel(opts, rt, a, src.as_deref(), &out, &pf)
    };
    let eng = opts.engine.label();
    let mode = if direct {
        format!(" — od razu do {}", p.container.ext().to_uppercase())
    } else if live {
        format!(" — DV/HDR10+ w locie do {}", p.container.ext().to_uppercase())
    } else {
        String::new()
    };
    panel.set(&a.filename, State::Nvenc, format!("{eng} {slot}/2 — {note} — dekoder: {decoder}{mode}"));

    let logf = File::create(&log)?;
    let mut cmd = crate::proc::command("ffmpeg");
    cmd.args(&args).stdin(Stdio::null()).stderr(Stdio::from(logf.try_clone()?));
    if live {
        cmd.stdout(Stdio::piped());
    } else {
        cmd.stdout(Stdio::from(logf));
    }
    let mut child = cmd.spawn()?;
    proc::register(child.id());
    let live = match fps {
        Some(fps) => match start_live(opts, a, &p, &part, &key, rt, fps, &mut child) {
            Ok(l) => Some(l),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                proc::unregister(child.id());
                return Err(e);
            }
        },
        None => None,
    };
    Ok(Running {
        child,
        live,
        name: a.filename.clone(),
        output: write_to,
        final_out: (direct || fps.is_some()).then(|| p.output.clone()),
        log,
        pf,
    })
}

/// Drugi ffmpeg (składanie pliku) i wątek wstrzykujący metadane między nim a koderem.
#[allow(clippy::too_many_arguments)]
fn start_live(
    opts: &Opts,
    a: &Analysis,
    p: &Paths,
    part: &Path,
    key: &str,
    rt: &Runtime,
    fps: (u64, u64),
    encoder: &mut Child,
) -> std::io::Result<Live> {
    let mux_log = rt.progress_dir.join(format!("{key}.mux.log"));
    let logf = File::create(&mux_log)?;
    let mut mux = crate::proc::command("ffmpeg")
        .args(live_mux_args(opts, a, p, part))
        .stdin(Stdio::piped())
        .stdout(Stdio::from(logf.try_clone()?))
        .stderr(Stdio::from(logf))
        .spawn()?;
    proc::register(mux.id());
    let (Some(enc_out), Some(mux_in)) = (encoder.stdout.take(), mux.stdin.take()) else {
        let _ = mux.kill();
        return Err(std::io::Error::other("brak potoku między koderem a ffmpeg"));
    };
    let rpu = (a.has_dovi && !opts.no_dv).then(|| p.temp_rpu.clone());
    let json = (a.has_hdr10plus && !opts.no_hdr10plus).then(|| p.temp_hdr10plus.clone());
    // Wynik kodera ma zawsze znaczniki PQ (smpte2084) — zgodność warstwy bazowej DV jak dla HDR10.
    let info = crate::inject::StreamInfo { fps, hlg: false };
    let l5 = scale_target(opts, a).map(|s| s.factors());
    let thread = thread::spawn(move || {
        let meta = crate::inject::Metadata::load(rpu.as_deref(), json.as_deref(), l5)?;
        let out = std::io::BufWriter::with_capacity(1 << 20, mux_in);
        let mut stats = crate::inject::run(enc_out, out, &meta, info)?;
        stats.converted_p7 = meta.converted_p7;
        Ok(stats)
    });
    Ok(Live { mux, thread, mux_log })
}

/// Koniec trybu w locie: wątek wstrzykujący, potem ffmpeg składający plik (kończy się po EOF).
fn finish_live(name: &str, live: Live) -> bool {
    let Live { mut mux, thread, mux_log } = live;
    let injected = thread.join().unwrap_or_else(|_| Err(anyhow::anyhow!("wątek wstrzykujący przerwany")));
    let mux_ok = mux.wait().map(|s| s.success()).unwrap_or(false);
    proc::unregister(mux.id());
    let ok = match injected {
        Ok(st) => {
            let dv = if st.rpus > 0 { format!(", Dolby Vision poziom {}", st.dv_level) } else { String::new() };
            let h10 = if st.hdr10plus > 0 { ", HDR10+" } else { "" };
            plog!(" -> [{name}] metadane w locie: {} klatek {}x{}{dv}{h10}", st.frames, st.width, st.height);
            if st.converted_p7 {
                plog!("    RPU profilu 7 przerobione na 8.1 (kodowana jest tylko warstwa bazowa)");
            }
            for w in st.warnings() {
                plog!("    UWAGA: {w}");
            }
            true
        }
        Err(e) => {
            plog!(" -> [{name}] BŁĄD wstrzykiwania DV/HDR10+: {e:#}");
            false
        }
    };
    if !mux_ok {
        plog!(" -> [{name}] BŁĄD ffmpeg składającego plik:");
        if let Ok(log) = fs::read_to_string(&mux_log) {
            let lines: Vec<&str> = log.lines().collect();
            for l in &lines[lines.len().saturating_sub(12)..] {
                plog!("    {l}");
            }
        }
    }
    ok && mux_ok
}

fn finish(opts: &Opts, mut r: Running, status: ExitStatus, panel: &Panel) {
    proc::unregister(r.child.id());
    let live = r.live.is_some();
    let live_ok = match r.live.take() {
        Some(l) => finish_live(&r.name, l),
        None => true,
    };

    if proc::cancelled() {
        // Częściowy plik nie może zostać uznany za ukończony etap przy resume.
        let _ = fs::remove_file(&r.output);
        panel.set(&r.name, State::Waiting, "Przerwano");
        return;
    }
    if status.success() && live_ok && result_valid(opts, &r.output, &r.pf) {
        if let Some(fin) = &r.final_out {
            // Tryb bezpośredni: gotowy plik wynikowy dopiero po pełnym zapisie (.part → nazwa docelowa).
            if let Err(e) = fs::rename(&r.output, fin) {
                panel.set(&r.name, State::Error, format!("nie można zapisać {}: {e}", fin.display()));
                plog!(" -> ffmpeg [{}]: BŁĄD zapisu pliku wynikowego: {e}", r.name);
                return;
            }
            let note = if live { "Zakodowano z DV/HDR10+ w locie do pliku wynikowego" } else { "Zakodowano od razu do pliku wynikowego" };
            panel.set(&r.name, State::NvencDone, note);
            plog!(" -> ffmpeg [{}]: ZAKOŃCZONY + PLIK OK (bez plików tymczasowych)", r.name);
        } else {
            panel.set(&r.name, State::NvencDone, format!("{} zakończony i zweryfikowany", opts.engine.label()));
            plog!(" -> ffmpeg [{}]: ZAKOŃCZONY + PLIK OK", r.name);
        }
    } else {
        panel.set(
            &r.name,
            State::Error,
            format!("{} nie utworzył poprawnego pliku ({status})", opts.engine.label()),
        );
        plog!(" -> ffmpeg [{}]: BŁĄD kodowania ({status})", r.name);
        if let Ok(log) = fs::read_to_string(&r.log) {
            let lines: Vec<&str> = log.lines().collect();
            let from = lines.len().saturating_sub(12);
            for l in &lines[from..] {
                plog!("    {l}");
            }
        }
        let _ = fs::remove_file(&r.output);
    }
}

fn reap(opts: &Opts, running: &mut Vec<Running>, panel: &Panel) {
    let mut i = 0;
    while i < running.len() {
        match running[i].child.try_wait() {
            Ok(Some(status)) => {
                let r = running.remove(i);
                finish(opts, r, status, panel);
            }
            Ok(None) => i += 1,
            Err(_) => {
                running.remove(i);
            }
        }
    }
}

/// Przerwanie: zabijamy ffmpeg i kasujemy niedokończone pliki.
fn abort_all(running: &mut Vec<Running>, panel: &Panel) {
    for r in running.iter_mut() {
        let _ = r.child.kill();
        let _ = r.child.wait();
        proc::unregister(r.child.id());
        if let Some(mut l) = r.live.take() {
            let _ = l.mux.kill();
            let _ = l.mux.wait();
            proc::unregister(l.mux.id());
            let _ = l.thread.join();
        }
        let _ = fs::remove_file(&r.output);
        panel.set(&r.name, State::Waiting, "Przerwano");
    }
    running.clear();
    if let Ok(mut g) = PIDS.lock() {
        g.clear();
    }
}

/// Dwa równoległe sloty kodowania; dekodowanie sprzętowe tej samej karty (NVDEC / QSV / VAAPI).
pub fn run_stage(opts: &Opts, rt: &Runtime, files: &[Analysis], panel: &Panel) {
    panel.draw(&format!("ETAP 2/3 — DUAL {}", opts.engine.label()));
    let mut running: Vec<Running> = vec![];

    for a in files {
        if proc::cancelled() {
            abort_all(&mut running, panel);
            return;
        }
        let p = opts.paths_for(&a.input_file);
        if p.output.exists() {
            panel.set(&a.filename, State::Done, "Plik wynikowy już istnieje");
            continue;
        }
        // Wznowienie po częściowym etapie 3: wideo jest już po wstrzyknięciu HDR10+ / Dolby Vision
        // (plik po NVENC/QSV został usunięty jako zbędny).
        if (p.step1.exists() || p.step2.exists()) && !opts.direct_encode(a.has_dovi, a.has_hdr10plus) {
            panel.set(&a.filename, State::NvencDone, "Kodowanie pominięte — wideo z metadanymi HDR już gotowe");
            continue;
        }
        if p.temp_video.exists() && !opts.direct_encode(a.has_dovi, a.has_hdr10plus) {
            if existing_temp_valid(opts, &p.temp_video, &a.input_file) {
                panel.set(
                    &a.filename,
                    State::NvencDone,
                    "Kodowanie pominięte — istnieje zweryfikowany plik tymczasowy",
                );
                continue;
            }
            plog!("    UWAGA: usuwam niekompletny plik tymczasowy: {}", p.temp_video.display());
            let _ = fs::remove_file(&p.temp_video);
        }

        while running.len() >= 2 {
            reap(opts, &mut running, panel);
            if proc::cancelled() {
                abort_all(&mut running, panel);
                return;
            }
            if running.len() >= 2 {
                thread::sleep(Duration::from_millis(500));
            }
        }

        let slot = running.len() + 1;
        match start(opts, rt, a, slot, panel) {
            Ok(r) => {
                plog!(" -> [{}] PID {} — start", a.filename, r.child.id());
                running.push(r);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                plog!(" -> [{}] POMINIĘTO: {e}", a.filename);
                panel.set(&a.filename, State::Error, e.to_string());
            }
            Err(e) => {
                panel.set(&a.filename, State::Error, format!("nie można uruchomić ffmpeg: {e}"));
            }
        }
        thread::sleep(Duration::from_millis(300));
    }

    while !running.is_empty() {
        reap(opts, &mut running, panel);
        if proc::cancelled() {
            abort_all(&mut running, panel);
            return;
        }
        if !running.is_empty() {
            thread::sleep(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod scale_tests {
    use super::fit_1080;

    #[test]
    fn fits_into_1080p() {
        assert_eq!(fit_1080(3840, 2160), Some((1920, 1080)));
        assert_eq!(fit_1080(3840, 1600), Some((1920, 800)));
        assert_eq!(fit_1080(3832, 1600), Some((1920, 802)));
        assert_eq!(fit_1080(2880, 2160), Some((1440, 1080)));
        assert_eq!(fit_1080(4096, 1716), Some((1920, 804)));
        assert_eq!(fit_1080(1920, 1080), None);
        assert_eq!(fit_1080(1280, 720), None);
    }
}
