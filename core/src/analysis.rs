use crate::{
    config::*,
    model::{hms, read_progress, Panel},
    probe::{self, ColorProfile},
    proc,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
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

/// Jeden odczyt pliku dla wszystkich narzędzi: ffmpeg (kopia → Annex B HEVC) na stdout, a wątek
/// rozsyła strumień do stdin każdego narzędzia (dovi_tool, hdr10plus_tool) — przy DV i HDR10+ plik
/// jest czytany raz, nie dwa razy. Postęp z `ffmpeg -progress` (pozycja w filmie względem długości)
/// trafia do okna analizy: procent od `from` do `to`, czas, prędkość i szacowany koniec.
#[allow(clippy::too_many_arguments)]
fn extract_hevc(
    input: &Path,
    tools: &[(&str, Vec<String>)],
    duration: Option<f64>,
    panel: &Panel,
    name: &str,
    step: &str,
    from: f64,
    to: f64,
) {
    use std::io::{Read, Write};
    let pf = std::env::temp_dir().join(format!(
        "videopipeline-{}-{}.ffprogress",
        std::process::id(),
        name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>()
    ));
    let _ = fs::remove_file(&pf);
    let Ok(mut ff) = crate::proc::command("ffmpeg")
        .args(["-hide_banner", "-loglevel", "quiet", "-nostats", "-progress"])
        .arg(&pf)
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0", "-c:v", "copy", "-bsf:v", "hevc_mp4toannexb", "-f", "hevc", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    proc::register(ff.id());
    let mut children = vec![];
    let mut sinks = vec![];
    for (tool, args) in tools {
        match crate::proc::command(tool)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut c) => {
                proc::register(c.id());
                sinks.push(c.stdin.take());
                children.push(c);
            }
            Err(e) => plog!("    BŁĄD: nie można uruchomić {tool}: {e}"),
        }
    }
    // Rozdzielacz: narzędzie, które skończy wcześniej (np. hdr10plus_tool bez metadanych), odpada,
    // reszta czyta dalej. Zamknięcie stdin = koniec danych dla narzędzia.
    let tee = ff.stdout.take().map(|mut src| {
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = match src.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for s in sinks.iter_mut() {
                    if let Some(w) = s {
                        if w.write_all(&buf[..n]).is_err() {
                            *s = None;
                        }
                    }
                }
                if sinks.iter().all(Option::is_none) {
                    break;
                }
            }
        })
    });

    let started = std::time::Instant::now();
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    loop {
        if proc::cancelled() {
            let _ = ff.kill();
            for c in children.iter_mut() {
                let _ = c.kill();
            }
        }
        let tools_done = children.iter_mut().all(|c| matches!(c.try_wait(), Ok(Some(_))));
        if tools_done && tee.as_ref().is_none_or(|t| t.is_finished()) {
            break;
        }
        if last.elapsed() >= std::time::Duration::from_millis(500) {
            last = std::time::Instant::now();
            let pr = read_progress(&pf);
            let cur = pr.out_us.map(|u| u / 1e6).unwrap_or(0.0);
            let note = match duration.filter(|d| *d > 0.0) {
                Some(d) => {
                    let frac = (cur / d).clamp(0.0, 1.0);
                    let speed = pr.speed.clone().filter(|v| v.ends_with('x')).unwrap_or_default();
                    let sx = speed.trim_end_matches('x').trim().parse::<f64>().unwrap_or(0.0);
                    let eta = if sx > 0.0 { format!(" · zostało ~{}", hms((d - cur).max(0.0) / sx)) } else { String::new() };
                    format!(
                        "Odczyt strumienia wideo: {:.0}% ({} / {}){}{eta}",
                        frac * 100.0,
                        hms(cur),
                        hms(d),
                        if speed.is_empty() { String::new() } else { format!(" · {speed}") }
                    )
                }
                None => format!("Odczyt strumienia wideo: {} (czas trwania nieznany)", hms(cur)),
            };
            let pct = duration
                .filter(|d| *d > 0.0)
                .map(|d| from + (to - from) * (cur / d).clamp(0.0, 1.0))
                .unwrap_or(from + (to - from) * (1.0 - (-started.elapsed().as_secs_f64() / 600.0).exp()));
            panel.draw_analysis(name, pct, step, &note);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = ff.kill();
    let _ = ff.wait();
    proc::unregister(ff.id());
    for mut c in children {
        let _ = c.wait();
        proc::unregister(c.id());
    }
    if let Some(t) = tee {
        let _ = t.join();
    }
    let _ = fs::remove_file(&pf);
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
            crate::proc::command("ffmpeg")
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
    let p = opts.paths_for(input);

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
        // Dolby Vision i HDR10+ z jednego odczytu pliku (patrz extract_hevc).
        let mut tools: Vec<(&str, Vec<String>)> = vec![];
        let mut what: Vec<&str> = vec![];
        if opts.no_dv {
            plog!("    Dolby Vision: pominięte (opcja -pdv)");
        } else {
            let _ = fs::remove_file(&p.temp_rpu);
            tools.push(("dovi_tool", vec!["extract-rpu".into(), "-".into(), "-o".into(), p.temp_rpu.to_string_lossy().into()]));
            what.push("DOLBY VISION RPU");
        }
        if opts.no_hdr10plus {
            plog!("    HDR10+: pominięte (opcja -ph)");
        } else {
            let _ = fs::remove_file(&p.temp_hdr10plus);
            tools.push(("hdr10plus_tool", vec!["extract".into(), "-o".into(), p.temp_hdr10plus.to_string_lossy().into(), "-".into()]));
            what.push("HDR10+");
        }
        if tools.is_empty() {
            panel.draw_analysis(&filename, 60.0, "DV / HDR10+", "Pominięte (opcje: bez Dolby Vision i HDR10+)");
        } else {
            let step = what.join(" + ");
            panel.draw_analysis(&filename, 22.0, &step, "Uruchamiam odczyt strumienia wideo…");
            extract_hevc(input, &tools, media.duration, panel, &filename, &step, 22.0, 60.0);
        }
        if !opts.no_dv {
            if fs::metadata(&p.temp_rpu).map(|m| m.len() > 0).unwrap_or(false) {
                has_dovi = true;
            } else {
                let _ = fs::remove_file(&p.temp_rpu);
            }
        }
        if !opts.no_hdr10plus {
            match fs::metadata(&p.temp_hdr10plus) {
                Ok(m) if m.len() > 10 => has_hdr10plus = true,
                Ok(_) => {
                    let _ = fs::remove_file(&p.temp_hdr10plus);
                }
                Err(_) => {}
            }
        }
        plog!(
            "    Dolby Vision: {} · HDR10+: {}",
            if opts.no_dv { "pominięte" } else if has_dovi { "jest" } else { "brak" },
            if opts.no_hdr10plus { "pominięte" } else if has_hdr10plus { "jest" } else { "brak" }
        );
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
