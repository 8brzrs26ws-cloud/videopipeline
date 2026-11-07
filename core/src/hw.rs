//! Wykrywanie sprzętowych enkoderów: NVIDIA NVENC, Intel QSV i Intel VAAPI.
//! Sama obecność enkodera w `ffmpeg -encoders` nic nie gwarantuje (brak karty, sterownika,
//! runtime'u oneVPL), więc każdy kandydat przechodzi krótkie, prawdziwe kodowanie testowe.
use crate::config::{Codec, Engine};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Default)]
pub struct HwInfo {
    /// ffmpeg uruchomił się i podał listę enkoderów.
    pub ffmpeg_ok: bool,
    /// Pary (silnik, kodek), które przeszły kodowanie testowe — w kolejności Engine::ALL.
    pub avail: Vec<(Engine, Codec)>,
    /// Węzeł renderD karty Intel (np. /dev/dri/renderD129).
    pub intel_dev: Option<PathBuf>,
}

impl HwInfo {
    pub fn has(&self, e: Engine, c: Codec) -> bool {
        self.avail.contains(&(e, c))
    }

    pub fn has_engine(&self, e: Engine) -> bool {
        self.avail.iter().any(|(x, _)| *x == e)
    }

    pub fn has_intel(&self) -> bool {
        self.has_engine(Engine::Qsv) || self.has_engine(Engine::Vaapi)
    }

    /// Intel dla kodeka: QSV, a gdy nie działa — VAAPI.
    pub fn intel_engine(&self, c: Codec) -> Option<Engine> {
        [Engine::Qsv, Engine::Vaapi].into_iter().find(|e| self.has(*e, c))
    }

    /// Silnik domyślny: NVIDIA, gdy w systemie działa NVENC (Intel trzeba wtedy wskazać jawnie);
    /// bez NVIDII — Intel (QSV, zapasowo VAAPI).
    pub fn default_engine(&self, c: Codec) -> Result<Engine, String> {
        if self.has_engine(Engine::Nvenc) {
            if self.has(Engine::Nvenc, c) {
                return Ok(Engine::Nvenc);
            }
            let hint = if self.intel_engine(c).is_some() {
                format!(" Karta Intel obsługuje {} — użyj --intel.", c.label())
            } else {
                String::new()
            };
            return Err(format!("Karta NVIDIA nie obsługuje kodowania {} (NVENC).{hint}", c.label()));
        }
        self.intel_engine(c)
            .ok_or_else(|| format!("Nie wykryto sprzętowego enkodera {} ({}).", c.label(), self.summary()))
    }

    /// Wszystkie działające kombinacje kodek + silnik (do przełączania w F7).
    pub fn combos(&self) -> Vec<(Engine, Codec)> {
        self.avail.clone()
    }

    /// Np. "NVIDIA NVENC (HEVC, AV1) · Intel QSV (HEVC)".
    pub fn summary(&self) -> String {
        if !self.ffmpeg_ok {
            return "ffmpeg nie działa".into();
        }
        let parts: Vec<String> = Engine::ALL
            .iter()
            .filter(|e| self.has_engine(**e))
            .map(|e| {
                let codecs: Vec<&str> = self
                    .avail
                    .iter()
                    .filter(|(x, _)| x == e)
                    .map(|(_, c)| c.label())
                    .collect();
                format!("{} {} ({})", e.vendor(), e.label(), codecs.join(", "))
            })
            .collect();
        if parts.is_empty() {
            "brak enkoderów GPU".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// Węzeł renderD karty Intel (vendor 0x8086). Przy kilku kartach Intel — pierwszy.
pub fn intel_render_node() -> Option<PathBuf> {
    let mut nodes: Vec<String> = fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("renderD"))
        .filter(|n| {
            fs::read_to_string(format!("/sys/class/drm/{n}/device/vendor"))
                .map(|v| v.trim() == "0x8086")
                .unwrap_or(false)
        })
        .collect();
    nodes.sort();
    nodes.first().map(|n| PathBuf::from(format!("/dev/dri/{n}")))
}

fn sv(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

/// Argumenty ffmpeg krótkiego kodowania testowego (lavfi → enkoder → null).
fn test_args(e: Engine, c: Codec, dev: Option<&Path>) -> Option<Vec<String>> {
    let mut v = sv(&["-hide_banner", "-nostdin", "-loglevel", "error"]);
    let dev = dev.map(|d| d.to_string_lossy().to_string());
    match e {
        Engine::Nvenc => {}
        Engine::Qsv => {
            let d = dev?;
            v.extend(sv(&["-init_hw_device", &format!("vaapi=va:{d}"), "-init_hw_device", "qsv=qs@va"]));
            v.extend(sv(&["-filter_hw_device", "qs"]));
        }
        Engine::Vaapi => {
            let d = dev?;
            v.extend(sv(&["-init_hw_device", &format!("vaapi=va:{d}"), "-filter_hw_device", "va"]));
        }
    }
    v.extend(sv(&["-f", "lavfi", "-i", "color=c=black:s=320x240:r=25:d=0.3", "-frames:v", "5", "-an"]));
    match e {
        Engine::Nvenc => v.extend(sv(&["-pix_fmt", "yuv420p"])),
        Engine::Qsv => v.extend(sv(&["-vf", "format=nv12,hwupload=extra_hw_frames=16"])),
        Engine::Vaapi => v.extend(sv(&["-vf", "format=nv12,hwupload"])),
    }
    v.extend(sv(&["-c:v", e.encoder(c), "-f", "null", "-"]));
    Some(v)
}

/// Uruchamia ffmpeg z limitem czasu; true = kod wyjścia 0.
fn run_with_timeout(args: &[String]) -> bool {
    let Ok(mut child) = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if start.elapsed() < TEST_TIMEOUT => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Kodowanie testowe jednej pary silnik + kodek.
pub fn test(e: Engine, c: Codec, intel_dev: Option<&Path>) -> bool {
    test_args(e, c, intel_dev).map(|a| run_with_timeout(&a)).unwrap_or(false)
}

/// Lista enkoderów ffmpeg (druga kolumna `ffmpeg -encoders`); None = ffmpeg nie działa.
fn ffmpeg_encoders() -> Option<Vec<String>> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
            .collect(),
    )
}

/// Wykrywa działające enkodery (testy równolegle, zwykle ~1 s).
pub fn detect() -> HwInfo {
    let Some(encoders) = ffmpeg_encoders() else {
        return HwInfo::default();
    };
    let intel_dev = intel_render_node();

    let mut jobs = vec![];
    for e in Engine::ALL {
        if e.is_intel() && intel_dev.is_none() {
            continue;
        }
        for c in [Codec::Hevc, Codec::Av1] {
            if !encoders.iter().any(|x| x == e.encoder(c)) {
                continue;
            }
            let dev = intel_dev.clone();
            jobs.push(((e, c), thread::spawn(move || test(e, c, dev.as_deref()))));
        }
    }
    let avail = jobs
        .into_iter()
        .filter_map(|(k, h)| h.join().unwrap_or(false).then_some(k))
        .collect();
    HwInfo { ffmpeg_ok: true, avail, intel_dev }
}
