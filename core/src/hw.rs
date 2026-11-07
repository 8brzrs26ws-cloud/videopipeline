//! Wykrywanie sprzętowych enkoderów: NVIDIA NVENC, Intel QSV i Intel VAAPI.
//! Sama obecność enkodera w `ffmpeg -encoders` nic nie gwarantuje (brak karty, sterownika,
//! runtime'u oneVPL), więc każdy kandydat przechodzi krótkie, prawdziwe kodowanie testowe.
use crate::config::{Codec, Engine};
use std::{
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

/// Windows: sposoby utworzenia urządzenia QSV, próbowane po kolei przy wykrywaniu.
/// 1) oneVPL sam wybiera kartę Intel (dispatcher widzi tylko runtime Intela, więc NVIDIA nie przeszkadza),
///    z D3D11 jako urządzeniem pomocniczym; 2) najpierw D3D11 na karcie o vendor_id 0x8086, potem QSV z niego.
pub const WIN_QSV_DEVICES: [&str; 2] =
    ["qsv=qs:hw_any,child_device_type=d3d11va", "d3d11va=dx:,vendor_id=0x8086"];

/// Wybrany (działający) sposób — ustawiany przez wykrywanie, używany przez preflight i kodowanie.
static WIN_QSV_CHOSEN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Windows nie ma węzłów /dev/dri: „węzeł” to opis urządzenia QSV dla -init_hw_device.
/// Czy karta naprawdę jest, rozstrzyga kodowanie testowe.
#[cfg(windows)]
pub fn intel_render_node() -> Option<PathBuf> {
    let chosen = WIN_QSV_CHOSEN.lock().ok().and_then(|g| g.clone());
    Some(PathBuf::from(chosen.unwrap_or_else(|| WIN_QSV_DEVICES[0].to_string())))
}

/// Argumenty ffmpeg tworzące urządzenie QSV „qs” na karcie Intel
/// (Linux: przez VA-API na węźle renderD, Windows: według opisu z WIN_QSV_DEVICES).
pub fn qsv_init_args(dev: &str) -> Vec<String> {
    if cfg!(windows) {
        if dev.starts_with("d3d11va") {
            sv(&["-init_hw_device", dev, "-init_hw_device", "qsv=qs@dx"])
        } else {
            sv(&["-init_hw_device", dev])
        }
    } else {
        sv(&["-init_hw_device", &format!("vaapi=va:{dev}"), "-init_hw_device", "qsv=qs@va"])
    }
}

/// Windows: próbuje kolejnych sposobów utworzenia QSV (test HEVC QSV) i zapamiętuje pierwszy działający.
/// Zwraca próby: (opis urządzenia, sukces, polecenie, błąd ffmpeg).
fn pick_windows_qsv_device(encoders: &[String]) -> Vec<(String, bool, String, String)> {
    let mut tries = vec![];
    if !cfg!(windows) || !encoders.iter().any(|x| x == "hevc_qsv") {
        return tries;
    }
    for d in WIN_QSV_DEVICES {
        let dev = PathBuf::from(d);
        let Some(args) = test_args(Engine::Qsv, Codec::Hevc, Some(&dev)) else { continue };
        let (ok, err) = run_with_timeout(&args);
        tries.push((d.to_string(), ok, format!("ffmpeg {}", args.join(" ")), err));
        if ok {
            if let Ok(mut g) = WIN_QSV_CHOSEN.lock() {
                *g = Some(d.to_string());
            }
            break;
        }
    }
    tries
}

/// Węzeł renderD karty Intel (vendor 0x8086). Przy kilku kartach Intel — pierwszy.
#[cfg(not(windows))]
pub fn intel_render_node() -> Option<PathBuf> {
    let mut nodes: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("renderD"))
        .filter(|n| {
            std::fs::read_to_string(format!("/sys/class/drm/{n}/device/vendor"))
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
            v.extend(qsv_init_args(&dev?));
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

/// Uruchamia ffmpeg z limitem czasu: (kod wyjścia 0?, komunikaty ffmpeg ze stderr).
fn run_with_timeout(args: &[String]) -> (bool, String) {
    let mut child = match Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (false, format!("nie można uruchomić ffmpeg: {e}")),
    };
    // stderr czytany w osobnym wątku — pełny bufor potoku nie zablokuje ffmpeg.
    let err_reader = child.stderr.take().map(|mut e| {
        thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut e, &mut s);
            s
        })
    });
    let start = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if start.elapsed() < TEST_TIMEOUT => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let mut err = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if !ok && start.elapsed() >= TEST_TIMEOUT {
        err.push_str("\n(przekroczony limit czasu testu)");
    }
    (ok, err.trim().to_string())
}

/// Kodowanie testowe jednej pary silnik + kodek.
pub fn test(e: Engine, c: Codec, intel_dev: Option<&Path>) -> bool {
    test_args(e, c, intel_dev).map(|a| run_with_timeout(&a).0).unwrap_or(false)
}

/// Wynik testu do `--detect`: silnik, kodek, sukces, polecenie ffmpeg, komunikaty ffmpeg.
pub struct TestReport {
    pub engine: Engine,
    pub codec: Codec,
    pub ok: bool,
    pub command: String,
    pub error: String,
}

/// Szczegółowe wykrywanie: każdy kandydat z poleceniem i błędem ffmpeg (diagnostyka);
/// na Windows także próby utworzenia urządzenia QSV.
pub fn detect_report() -> (HwInfo, Vec<TestReport>, Vec<(String, bool, String, String)>) {
    let Some(encoders) = ffmpeg_encoders() else {
        return (HwInfo::default(), vec![], vec![]);
    };
    let qsv_tries = pick_windows_qsv_device(&encoders);
    let intel_dev = intel_render_node();
    let mut jobs = vec![];
    for e in Engine::ALL {
        if cfg!(windows) && e == Engine::Vaapi {
            continue;
        }
        for c in [Codec::Hevc, Codec::Av1] {
            let enc = e.encoder(c);
            if !encoders.iter().any(|x| x == enc) {
                jobs.push((e, c, None));
                continue;
            }
            let args = test_args(e, c, intel_dev.as_deref());
            let h = args.clone().map(|a| thread::spawn(move || run_with_timeout(&a)));
            jobs.push((e, c, Some((args, h))));
        }
    }
    let mut reports = vec![];
    let mut avail = vec![];
    for (e, c, job) in jobs {
        let (ok, command, error) = match job {
            None => (false, String::new(), format!("ffmpeg nie ma enkodera {}", e.encoder(c))),
            Some((None, _)) => (false, String::new(), "nie znaleziono karty Intel".into()),
            Some((Some(args), h)) => {
                let (ok, err) = h.and_then(|h| h.join().ok()).unwrap_or((false, String::new()));
                (ok, format!("ffmpeg {}", args.join(" ")), err)
            }
        };
        if ok {
            avail.push((e, c));
        }
        reports.push(TestReport { engine: e, codec: c, ok, command, error });
    }
    (HwInfo { ffmpeg_ok: true, avail, intel_dev }, reports, qsv_tries)
}

/// Wersja ffmpeg (pierwsza linia `ffmpeg -version`).
pub fn ffmpeg_version() -> Option<String> {
    let out = Command::new("ffmpeg").arg("-version").stdin(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(str::to_string)
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
    pick_windows_qsv_device(&encoders);
    let intel_dev = intel_render_node();

    let mut jobs = vec![];
    for e in Engine::ALL {
        if e.is_intel() && intel_dev.is_none() {
            continue;
        }
        // VA-API istnieje tylko na Linuksie; na Windows Intel koduje przez QSV (D3D11).
        if cfg!(windows) && e == Engine::Vaapi {
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
