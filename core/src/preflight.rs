use crate::config::*;
use anyhow::{bail, Result};
use std::{fs, path::Path, process::Command};

fn ffmpeg_text(args: &[&str]) -> String {
    match Command::new("ffmpeg").args(args).output() {
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
    let ok = Command::new("ffmpeg")
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

pub fn run(opts: &Opts) -> Result<Runtime> {
    let progress_dir =
        std::env::temp_dir().join(format!("dual_nvenc_pipeline.{}", std::process::id()));
    fs::create_dir_all(&progress_dir)?;
    let mut rt = Runtime { av1_tune: "hq".into(), denoise: Denoise::Off, progress_dir };

    plog!(
        "Kopiowanie -c: {}",
        if opts.copy {
            "WŁĄCZONE — wynik zostanie skopiowany do source/encoded i sprawdzony SHA-512."
        } else {
            "wyłączone — wynik pozostanie wyłącznie w katalogu wyjściowym."
        }
    );

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
        plog!("UWAGA: tryb AV1 nie wstrzykuje Dolby Vision ani HDR10+.");
    } else {
        plog!("Kodek wideo: HEVC NVENC — CQ {}.", opts.hevc_cq);
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
    Ok(rt)
}
