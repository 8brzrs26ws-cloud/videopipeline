use crate::{
    config::*,
    model::Panel,
    probe::{self, ColorProfile},
    proc,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Analysis {
    pub input_file: PathBuf,
    pub filename: String,
    pub stem: String,
    pub codec: String,
    pub has_dovi: bool,
    pub has_hdr10plus: bool,
    pub color: ColorProfile,
    pub pol_audio: Option<u32>,
    pub eng_audio: Option<u32>,
    pub first_audio: Option<u32>,
    pub pol_sub: Option<u32>,
    pub eng_sub: Option<u32>,
    /// np. "crop=1920:800:0:140"
    pub crop: Option<String>,
    /// Które etapy pominięto przy analizie (do wykrycia nieaktualnej analizy po zmianie opcji).
    #[serde(default)]
    pub skipped_crop: bool,
    #[serde(default)]
    pub skipped_dv: bool,
    #[serde(default)]
    pub skipped_hdr10plus: bool,
}

impl Analysis {
    /// Analiza pominęła coś, czego bieżące opcje już wymagają.
    pub fn stale_for(&self, o: &Opts) -> bool {
        (self.skipped_crop && !o.no_crop)
            || (self.skipped_dv && !o.no_dv)
            || (self.skipped_hdr10plus && !o.no_hdr10plus)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }
}

/// ffmpeg (copy -> Annex B HEVC) | narzędzie czytające ze stdin.
fn pipe_hevc(input: &Path, tool: &str, tool_args: &[&str]) {
    let Ok(mut ff) = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "quiet", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-c:v", "copy", "-bsf:v", "hevc_mp4toannexb", "-f", "hevc", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let ff_id = ff.id();
    proc::register(ff_id);
    let Some(out) = ff.stdout.take() else {
        proc::unregister(ff_id);
        return;
    };
    let _ = proc::run(Command::new(tool).args(tool_args).stdin(Stdio::from(out)));
    let _ = ff.kill();
    let _ = ff.wait();
    proc::unregister(ff_id);
}

fn last_crop(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .filter_map(|l| {
            let s = &l[l.rfind("crop=")? + 5..];
            let tok = s.split_whitespace().next()?;
            let nums: Vec<&str> = tok.split(':').collect();
            (nums.len() == 4 && nums.iter().all(|n| n.parse::<u32>().is_ok()))
                .then(|| tok.to_string())
        })
        .last()
}

fn detect_crop(
    input: &Path,
    duration: Option<f64>,
    orig_w: u32,
    name: &str,
    panel: &Panel,
) -> Option<String> {
    let mut dur = duration.unwrap_or(0.0) as u64;
    if dur <= 600 {
        dur = 7200;
    }
    let points: Vec<u64> = (15..=85).step_by(10).map(|p| dur * p / 100).collect();
    let mut found: Vec<String> = vec![];

    for (i, ts) in points.iter().enumerate() {
        let pct = 65.0 + 27.0 * (i + 1) as f64 / points.len() as f64;
        panel.draw_analysis(
            name,
            pct,
            "CROPDETECT",
            &format!("Próbka {}/{} — ffmpeg -ss {ts}", i + 1, points.len()),
        );
        if proc::cancelled() {
            break;
        }
        let Some(out) = proc::output(
            Command::new("ffmpeg")
                .args(["-hide_banner", "-ss", &ts.to_string(), "-i"])
                .arg(input)
                .args(["-vframes", "15", "-vf"])
                .arg("format=yuv420p,gblur=sigma=2,cropdetect=limit=24:round=16:reset=0")
                .args(["-f", "null", "-"]),
        ) else {
            continue;
        };
        if let Some(c) = last_crop(&String::from_utf8_lossy(&out.stderr)) {
            found.push(c);
        }
    }
    if found.is_empty() {
        return None;
    }

    let total = found.len();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in &found {
        *counts.entry(c.clone()).or_default() += 1;
    }

    // Największy kadr, który ma >= 90% szerokości i >= 10% głosów.
    let mut best: Option<(u64, u64, String)> = None;
    for (c, n) in &counts {
        let p: Vec<u64> = c.split(':').filter_map(|x| x.parse().ok()).collect();
        if p.len() != 4 {
            continue;
        }
        if p[0] as f64 >= orig_w as f64 * 0.90 && (*n as f64 / total as f64) >= 0.10 {
            let area = p[0] * p[1];
            if best.as_ref().map_or(true, |b| area > b.0) {
                best = Some((area, p[1], c.clone()));
            }
        }
    }
    let (_, h, c) = best?;
    (h < 2140).then(|| format!("crop={c}"))
}

pub fn analyze(opts: &Opts, input: &Path, panel: &mut Panel) -> Result<Analysis> {
    let filename = input.file_name().unwrap().to_string_lossy().to_string();
    let stem = input.file_stem().unwrap().to_string_lossy().to_string();
    let p = opts.paths(&stem);

    panel.draw_analysis(&filename, 8.0, "AUDIO / NAPISY", "Odczytuję ścieżki przez ffprobe");
    let media = probe::probe(input)?;

    let mut pol_audio: Option<u32> = None;
    let mut eng_audio: Option<u32> = None;
    for (idx, lang) in &media.audio {
        let l = lang.to_lowercase();
        if l.contains("pol") && pol_audio.is_none() {
            pol_audio = Some(*idx);
        } else if l.contains("eng") && eng_audio.is_none() {
            eng_audio = Some(*idx);
        }
    }
    let mut pol_sub: Option<u32> = None;
    let mut eng_sub: Option<u32> = None;
    for (idx, lang) in &media.subs {
        let l = lang.to_lowercase();
        if l.contains("pol") && pol_sub.is_none() {
            pol_sub = Some(*idx);
        } else if l.contains("eng") && eng_sub.is_none() {
            eng_sub = Some(*idx);
        }
    }

    panel.draw_analysis(&filename, 20.0, "PROFIL KOLORU", "Odczytuję głębię bitową i metadane");
    let color = media.color.clone();
    let (mut has_dovi, mut has_hdr10plus) = (false, false);

    if color.is_sdr_8bit {
        panel.draw_analysis(&filename, 60.0, "SDR 8-BIT", "DV RPU i HDR10+ pominięte — źródło SDR 8-bit");
    } else if opts.codec == Codec::Av1 {
        panel.draw_analysis(&filename, 60.0, "AV1 / HDR", "DV RPU i HDR10+ pominięte — to ścieżka HEVC");
    } else {
        if opts.no_dv {
            panel.draw_analysis(&filename, 42.0, "DOLBY VISION RPU", "Pominięte (opcja: bez analizy Dolby Vision)");
        } else {
            panel.draw_analysis(&filename, 22.0, "DOLBY VISION RPU", "Ekstrakcja RPU");
            pipe_hevc(input, "dovi_tool", &["extract-rpu", "-", "-o", &p.temp_rpu.to_string_lossy()]);
            if fs::metadata(&p.temp_rpu).map(|m| m.len() > 0).unwrap_or(false) {
                has_dovi = true;
            } else {
                let _ = fs::remove_file(&p.temp_rpu);
            }
        }

        if opts.no_hdr10plus {
            panel.draw_analysis(&filename, 60.0, "HDR10+", "Pominięte (opcja: bez analizy HDR10+)");
        } else {
            panel.draw_analysis(&filename, 45.0, "HDR10+", "Ekstrakcja metadanych HDR10+");
            pipe_hevc(
                input,
                "hdr10plus_tool",
                &["extract", "-o", &p.temp_hdr10plus.to_string_lossy(), "-"],
            );
            match fs::metadata(&p.temp_hdr10plus) {
                Ok(m) if m.len() > 10 => has_hdr10plus = true,
                Ok(_) => {
                    let _ = fs::remove_file(&p.temp_hdr10plus);
                }
                Err(_) => {}
            }
        }
    }

    let crop = if opts.no_crop {
        panel.draw_analysis(&filename, 92.0, "CROPDETECT", "Pominięte (opcja: bez crop)");
        None
    } else if has_dovi || has_hdr10plus {
        panel.draw_analysis(&filename, 92.0, "CROPDETECT", "Pominięte — zachowuję pełny obraz HDR/DV");
        None
    } else {
        detect_crop(input, media.duration, media.width.unwrap_or(3840), &filename, panel)
    };

    Ok(Analysis {
        input_file: input.to_path_buf(),
        filename,
        stem,
        codec: opts.codec.name().into(),
        has_dovi,
        has_hdr10plus,
        color,
        pol_audio,
        eng_audio,
        first_audio: media.audio.first().map(|a| a.0),
        pol_sub,
        eng_sub,
        crop,
        skipped_crop: opts.no_crop,
        skipped_dv: opts.no_dv,
        skipped_hdr10plus: opts.no_hdr10plus,
    })
}
