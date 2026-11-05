use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, process::Command};

#[derive(Deserialize, Default)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<StreamJson>,
    format: Option<FormatJson>,
}

#[derive(Deserialize, Default)]
struct StreamJson {
    index: u32,
    codec_type: Option<String>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    width: Option<u32>,
    #[serde(default)]
    tags: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
struct FormatJson {
    duration: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ColorProfile {
    pub pix_fmt: String,
    pub bits_per_raw_sample: String,
    pub primaries: String,
    pub trc: String,
    pub space: String,
    pub range: String,
    pub is_sdr_8bit: bool,
    pub sdr_primaries: String,
    pub sdr_trc: String,
    pub sdr_space: String,
    pub sdr_range: String,
}

pub struct Media {
    pub audio: Vec<(u32, String)>,
    pub subs: Vec<(u32, String)>,
    pub width: Option<u32>,
    pub duration: Option<f64>,
    pub color: ColorProfile,
}

fn known(s: &str) -> bool {
    !s.is_empty() && s != "unknown" && s != "unspecified"
}

fn color_from(s: &StreamJson) -> ColorProfile {
    let g = |o: &Option<String>| o.clone().unwrap_or_default();
    let mut c = ColorProfile {
        pix_fmt: g(&s.pix_fmt),
        bits_per_raw_sample: g(&s.bits_per_raw_sample),
        primaries: g(&s.color_primaries),
        trc: g(&s.color_transfer),
        space: g(&s.color_space),
        range: g(&s.color_range),
        sdr_primaries: "bt709".into(),
        sdr_trc: "bt709".into(),
        sdr_space: "bt709".into(),
        sdr_range: "tv".into(),
        ..Default::default()
    };
    let is_8bit = c.bits_per_raw_sample == "8"
        || matches!(
            c.pix_fmt.as_str(),
            "yuv420p" | "yuvj420p" | "nv12" | "yuv422p" | "yuvj422p" | "yuv444p"
                | "yuvj444p" | "gray" | "gray8" | "rgb24" | "bgr24" | "gbrp"
        );
    let is_hdr = matches!(c.trc.as_str(), "smpte2084" | "arib-std-b67");
    c.is_sdr_8bit = is_8bit && !is_hdr;
    if known(&c.primaries) {
        c.sdr_primaries = c.primaries.clone();
    }
    if known(&c.trc) {
        c.sdr_trc = c.trc.clone();
    }
    if known(&c.space) {
        c.sdr_space = c.space.clone();
    }
    match c.range.as_str() {
        "pc" | "jpeg" => c.sdr_range = "pc".into(),
        "tv" | "mpeg" => c.sdr_range = "tv".into(),
        _ => {}
    }
    c
}

pub fn probe(input: &Path) -> Result<Media> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-show_format", "-of", "json"])
        .arg(input)
        .output()
        .context("nie można uruchomić ffprobe")?;
    if !out.status.success() {
        bail!("ffprobe zakończył się błędem dla {}", input.display());
    }
    let j: ProbeJson = serde_json::from_slice(&out.stdout).context("zły JSON z ffprobe")?;
    let lang = |s: &StreamJson| s.tags.get("language").cloned().unwrap_or_default();

    let audio = j
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .map(|s| (s.index, lang(s)))
        .collect();
    let subs = j
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("subtitle"))
        .map(|s| (s.index, lang(s)))
        .collect();
    let video = j.streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));

    Ok(Media {
        audio,
        subs,
        width: video.and_then(|v| v.width),
        duration: j
            .format
            .and_then(|f| f.duration)
            .and_then(|d| d.parse::<f64>().ok()),
        color: video.map(color_from).unwrap_or_default(),
    })
}

fn probe_value(path: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(args)
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn video_codec(path: &Path) -> Option<String> {
    probe_value(path, &["-select_streams", "v:0", "-show_entries", "stream=codec_name"])
}

pub fn container_duration(path: &Path) -> Option<f64> {
    probe_value(path, &["-show_entries", "format=duration"])?.parse().ok()
}
