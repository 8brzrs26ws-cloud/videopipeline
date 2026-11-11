use crate::config::*;
use anyhow::{bail, Result};
use std::{fs, path::Path};

fn ffmpeg_text(args: &[&str]) -> String {
    match crate::proc::command("ffmpeg").args(args).output() {
        Ok(o) => format!(
            "{}\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(_) => String::new(),
    }
}

/// Krótki, prawdziwy encode na GPU — sama obecność w -encoders nic nie gwarantuje.
fn av1_selftest(tune: &str, cq: u8, out: &Path) -> bool {
    let _ = fs::remove_file(out);
    let ok = crate::proc::command("ffmpeg")
        .args(["-hide_banner", "-y", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "color=c=black:s=320x180:r=24:d=0.25"])
        .args(["-an", "-frames:v", "6", "-c:v", "av1_nvenc", "-preset", "p7"])
        .args(["-tune", tune, "-rc", "vbr", "-cq", &cq.to_string(), "-b:v", "0"])
        .args(["-spatial-aq", "1", "-temporal-aq", "1", "-bf", "4"])
        .args(["-b_ref_mode", "middle", "-multipass", "fullres"])
        .args(["-rc-lookahead", "32", "-highbitdepth", "1", "-pix_fmt", "yuv420p"])
        .arg(out)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let valid = ok
        && fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false)
        && crate::probe::video_codec(out).as_deref() == Some("av1");
    let _ = fs::remove_file(out);
    valid
}

/// NVIDIA: AV1 wymaga av1_nvenc z -highbitdepth; UHQ tylko gdy sterownik je przyjmie.
fn preflight_nvenc(opts: &Opts, rt: &mut Runtime) -> Result<()> {
    if opts.codec == Codec::Av1 {
        let enc = ffmpeg_text(&["-hide_banner", "-encoders"]);
        if !enc.lines().any(|l| l.split_whitespace().nth(1) == Some("av1_nvenc")) {
            bail!("Wybrano -av1, ale bieżący FFmpeg nie udostępnia enkodera av1_nvenc.");
        }
        let help = ffmpeg_text(&["-hide_banner", "-h", "encoder=av1_nvenc"]);
        if !help.contains("highbitdepth") {
            bail!("av1_nvenc w tym FFmpeg nie udostępnia opcji -highbitdepth.");
        }
        if help.split_whitespace().any(|w| w == "uhq") {
            rt.av1_tune = "uhq".into();
        }

        let test_out = std::env::temp_dir()
            .join(format!("videopipeline-av1-test.{}.mkv", std::process::id()));
        if !av1_selftest(&rt.av1_tune, opts.av1_cq, &test_out) && rt.av1_tune == "uhq" {
            plog!("UWAGA: AV1 UHQ nie przeszedł testu na bieżącym sterowniku — używam tune=hq.");
            rt.av1_tune = "hq".into();
        }
        if !av1_selftest(&rt.av1_tune, opts.av1_cq, &test_out) {
            bail!("test av1_nvenc nie powiódł się. Kodowanie nie zostanie rozpoczęte.");
        }
        plog!(
            "Kodek wideo: AV1 NVENC — preset p7, tune={}, VBR CQ {}, multipass fullres.",
            rt.av1_tune,
            opts.av1_cq
        );
        plog!("AV1 SDR 8-bit: wyjście AV1 10-bit przez -highbitdepth 1.");
    } else if opts.codec == Codec::Avc {
        plog!("Kodek wideo: AVC NVENC — profil High 8-bit, preset p7, VBR CQ {}, multipass fullres.", opts.avc_cq);
    } else {
        plog!("Kodek wideo: HEVC NVENC — CQ {}.", opts.hevc_cq);
    }
    Ok(())
}

/// Intel: karta (renderD), kodowanie testowe wybranego enkodera i lista dekoderów QSV.
fn preflight_intel(opts: &Opts, rt: &mut Runtime) -> Result<()> {
    let enc = opts.engine.encoder(opts.codec);
    let Some(dev) = crate::hw::intel_render_node() else {
        bail!("Nie znaleziono karty Intel — {enc} niedostępny.");
    };
    if !crate::hw::test(opts.engine, opts.codec, Some(&dev)) {
        bail!(
            "Test {enc} nie powiódł się ({}). Kodowanie nie zostanie rozpoczęte.",
            if cfg!(windows) { "sterownik karty Intel?" } else { "sterownik iHD / runtime oneVPL?" }
        );
    }
    if opts.engine == Engine::Qsv {
        rt.qsv_decoders = ffmpeg_text(&["-hide_banner", "-decoders"])
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1))
            .filter(|n| n.ends_with("_qsv"))
            .map(str::to_string)
            .collect();
    }
    let cq = opts.cq();
    let quality = match (opts.engine, opts.codec) {
        (Engine::Qsv, _) => format!("ICQ global_quality {cq}, preset {}", opts.qsv_preset),
        (_, Codec::Av1) => format!("CQP qp {} (CQ {cq} × 5, skala AV1 0–255)", (cq as u32 * 5).min(255)),
        _ => format!("CQP qp {cq}"),
    };
    let d = dev.to_string_lossy();
    let where_ = if !cfg!(windows) {
        d.to_string()
    } else if crate::hw::qsv_is_sysmem(&d) {
        "Windows, klatki przez pamięć systemową".to_string()
    } else {
        format!("Windows, {d}")
    };
    plog!("Kodek wideo: {} — {quality} — karta Intel ({where_}).", opts.codec_label());
    plog!(
        "Dekodowanie: {} (gdy kodek źródła nie jest obsługiwany sprzętowo — CPU).",
        if opts.engine == Engine::Qsv { "QSV" } else { "VAAPI" }
    );
    rt.intel_dev = Some(dev);
    Ok(())
}

pub fn run(opts: &Opts) -> Result<Runtime> {
    let progress_dir =
        std::env::temp_dir().join(format!("dual_nvenc_pipeline.{}", std::process::id()));
    fs::create_dir_all(&progress_dir)?;
    let mut rt = Runtime {
        av1_tune: "hq".into(),
        denoise: Denoise::Off,
        progress_dir,
        intel_dev: None,
        qsv_decoders: vec![],
        tonemap: Tonemap::None,
        vulkan_dev: String::new(),
    };

    plog!(
        "Kopiowanie -c: {}",
        if opts.copy {
            "WŁĄCZONE — wynik zostanie skopiowany do source/encoded i sprawdzony SHA-512."
        } else {
            "wyłączone — wynik pozostanie wyłącznie w katalogu wyjściowym."
        }
    );

    match opts.engine {
        Engine::Nvenc => preflight_nvenc(opts, &mut rt)?,
        Engine::Qsv | Engine::Vaapi => preflight_intel(opts, &mut rt)?,
    }
    if opts.codec == Codec::Av1 {
        plog!("UWAGA: tryb AV1 nie wstrzykuje Dolby Vision ani HDR10+.");
    }
    if opts.codec == Codec::Avc {
        plog!("AVC: 8-bit, tylko źródła SDR (pliki HDR są pomijane z komunikatem); bez Dolby Vision i HDR10+.");
    }

    if opts.pl_eac3 {
        plog!("Audio PL: E-AC-3 stereo 192 kb/s (pierwsza ścieżka language=pol).");
    }

    if opts.no_crop || opts.no_dv || opts.no_hdr10plus {
        let mut skipped: Vec<&str> = vec![];
        if opts.no_crop {
            skipped.push("cropdetect");
        }
        if opts.no_dv {
            skipped.push("Dolby Vision (RPU)");
        }
        if opts.no_hdr10plus {
            skipped.push("HDR10+");
        }
        plog!("Pomijane etapy analizy: {}.", skipped.join(", "));
    }

    if opts.extra {
        plog!("Tryb -da: polskie audio + angielskie audio (E-AC-3 5.1) + napisy EN i PL.");
    } else {
        plog!("Domyślnie: tylko polska ścieżka audio, bez napisów (angielskie audio i napisy: -da).");
    }

    if opts.denoise {
        let f = ffmpeg_text(&["-hide_banner", "-filters"]);
        let has = |n: &str| f.contains(n);
        match opts.engine {
            Engine::Nvenc => {
                if has("bilateral_cuda") && has("hwupload_cuda") && has("hwdownload") {
                    rt.denoise = Denoise::Cuda;
                    plog!("Denoiser -d: bilateral_cuda (GPU)");
                } else if has("hqdn3d") {
                    rt.denoise = Denoise::Cpu;
                    plog!("UWAGA: brak kompletnego zestawu filtrów CUDA — denoiser użyje hqdn3d na CPU.");
                } else {
                    bail!("opcja -d wymaga bilateral_cuda + hwupload_cuda + hwdownload albo hqdn3d.");
                }
            }
            Engine::Qsv => {
                rt.denoise = Denoise::Intel;
                plog!("Denoiser -d: vpp_qsv denoise={DENOISE_QSV_LEVEL} (GPU Intel)");
            }
            Engine::Vaapi => {
                if has("denoise_vaapi") {
                    rt.denoise = Denoise::Intel;
                    plog!("Denoiser -d: denoise_vaapi={DENOISE_VAAPI_LEVEL} (GPU Intel)");
                } else if has("hqdn3d") {
                    rt.denoise = Denoise::Cpu;
                    plog!("UWAGA: brak denoise_vaapi — denoiser użyje hqdn3d na CPU.");
                } else {
                    bail!("opcja -d wymaga denoise_vaapi albo hqdn3d.");
                }
            }
        }
    }
    if opts.codec == Codec::Avc {
        let (t, vk) = detect_tonemap(opts, &rt);
        rt.tonemap = t;
        rt.vulkan_dev = vk;
        match rt.tonemap {
            Tonemap::None => plog!("UWAGA: ffmpeg nie ma filtrów mapowania tonów — pliki HDR w AVC zostaną pominięte."),
            Tonemap::Placebo => plog!(
                "AVC ze źródeł HDR: mapowanie tonów HDR → SDR BT.709 — {} (urządzenie Vulkan: {}).",
                Tonemap::Placebo.label(),
                rt.vulkan_dev
            ),
            t => plog!("AVC ze źródeł HDR: mapowanie tonów HDR → SDR BT.709 — {}.", t.label()),
        }
    }
    Ok(rt)
}

/// Testowe źródło HDR10 (PQ, BT.2020, 10-bit) do sprawdzenia filtrów mapowania tonów.
const HDR_TEST_SRC: &str =
    "color=c=gray:s=320x240:r=25:d=0.2,format=yuv420p10le,setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc";

fn ffmpeg_ok(args: &[String]) -> bool {
    crate::proc::command("ffmpeg")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Najlepsze działające mapowanie tonów dla wybranej karty: libplacebo (Vulkan, jakość jak w mpv),
/// potem filtry sprzętowe karty, zapasowo CPU. Zwraca też urządzenie Vulkan dla libplacebo.
fn detect_tonemap(opts: &Opts, rt: &Runtime) -> (Tonemap, String) {
    let sv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let run = |pre: Vec<String>, vf: &str| {
        let mut a = sv(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
        a.extend(pre);
        a.extend(sv(&["-f", "lavfi", "-i", HDR_TEST_SRC, "-vf", vf, "-frames:v", "3", "-f", "null", "-"]));
        ffmpeg_ok(&a)
    };
    // libplacebo: najpierw karta, na której kodujemy (nazwa urządzenia Vulkan), potem pierwsze urządzenie.
    // VAAPI zostaje przy tonemap_vaapi — klatki muszą wrócić na kartę Intel przez urządzenie VAAPI.
    if opts.engine != Engine::Vaapi {
        let vf = crate::encode::tonemap_placebo(None, "yuv420p");
        let hint = if opts.engine == Engine::Nvenc { "NVIDIA" } else { "Intel" };
        for dev in [hint, "0"] {
            if run(crate::encode::vulkan_args(dev), &vf) {
                return (Tonemap::Placebo, dev.to_string());
            }
        }
    }
    let dev = rt.intel_dev.as_ref().map(|d| d.to_string_lossy().to_string());
    let gpu = match (opts.engine, dev) {
        (Engine::Nvenc, _) => run(sv(&["-init_hw_device", "opencl=ocl", "-filter_hw_device", "ocl"]), crate::encode::TONEMAP_OPENCL)
            .then_some(Tonemap::OpenCl),
        (Engine::Qsv, Some(d)) if !crate::hw::qsv_is_sysmem(&d) => {
            let mut pre = crate::hw::qsv_init_args(&d);
            pre.extend(sv(&["-filter_hw_device", "qs"]));
            let vf = format!("format=p010le,hwupload=extra_hw_frames=16,vpp_qsv={}:format=nv12", crate::encode::TONEMAP_QSV);
            run(pre, &vf).then_some(Tonemap::Qsv)
        }
        (Engine::Vaapi, Some(d)) => {
            let pre = sv(&["-init_hw_device", &format!("vaapi=va:{d}"), "-filter_hw_device", "va"]);
            run(pre, &format!("format=p010le,hwupload,{}", crate::encode::TONEMAP_VAAPI)).then_some(Tonemap::Vaapi)
        }
        _ => None,
    };
    let t = gpu.or_else(|| run(vec![], crate::encode::TONEMAP_CPU).then_some(Tonemap::Cpu)).unwrap_or(Tonemap::None);
    (t, String::new())
}
