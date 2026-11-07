//! Obciążenie karty Intel z /proc/<pid>/fdinfo (sterowniki i915 i xe) — bez roota i bez intel_gpu_top.
//! Liczone dla procesów ffmpeg uruchomionych przez videopipeline (rejestr PID-ów).
//!
//! Silniki Intela: VIDEO (VCS / VDBox) koduje i dekoduje — sprzętowo to ten sam blok, więc
//! obciążenie kodera i dekodera jest jedną wartością; VIDEO-ENHANCE (VECS) to filtry vpp
//! (crop, skalowanie, odszumianie); RENDER/COMPUTE — shadery.
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Default, Debug)]
pub struct IntelStats {
    pub name: String,
    /// VCS: kodowanie + dekodowanie (%)
    pub video: Option<f64>,
    /// VECS: vpp (%)
    pub enhance: Option<f64>,
    /// RCS/CCS: render + compute (%)
    pub render: Option<f64>,
    pub freq_mhz: Option<u32>,
    pub freq_max_mhz: Option<u32>,
    /// Pamięć karty (lub GTT) zajęta przez nasze procesy.
    pub mem_bytes: Option<u64>,
}

#[derive(Default, Clone)]
struct Client {
    /// klasa -> ns zajętości (i915) albo cykle (xe)
    busy: HashMap<&'static str, u64>,
    /// xe: drm-total-cycles (licznik odniesienia)
    total: HashMap<&'static str, u64>,
    cap: HashMap<&'static str, u64>,
    mem: u64,
}

pub struct IntelSampler {
    pdev: Option<String>,
    card: Option<PathBuf>,
    name: String,
    prev: HashMap<String, Client>,
    prev_t: Instant,
}

fn class_of(raw: &str) -> Option<&'static str> {
    match raw {
        "video" | "vcs" => Some("video"),
        "video-enhance" | "vecs" => Some("enhance"),
        "render" | "rcs" | "compute" | "ccs" => Some("render"),
        _ => None,
    }
}

/// "12345 ns" → 12345; "512 KiB" → bajty.
fn parse_value(v: &str) -> Option<u64> {
    let mut it = v.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    let mul = match it.next() {
        Some("KiB") => 1024,
        Some("MiB") => 1024 * 1024,
        Some("GiB") => 1024 * 1024 * 1024,
        _ => 1,
    };
    Some(n * mul)
}

fn read_u32(p: &Path) -> Option<u32> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

impl IntelSampler {
    /// `render` — węzeł /dev/dri/renderD* karty Intel (z wykrywania).
    pub fn new(render: Option<&Path>) -> Self {
        let node = render.and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string());
        let dev_dir = node.as_ref().and_then(|n| fs::canonicalize(format!("/sys/class/drm/{n}/device")).ok());
        let pdev = dev_dir.as_ref().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().to_string());

        // cardN tej samej karty — tam są częstotliwości.
        let card = dev_dir.as_ref().and_then(|dd| {
            fs::read_dir("/sys/class/drm").ok()?.filter_map(|e| e.ok()).find_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                let is_card = n.starts_with("card") && !n.contains('-');
                let same = fs::canonicalize(e.path().join("device")).ok().as_ref() == Some(dd);
                (is_card && same).then(|| e.path())
            })
        });

        let id = dev_dir
            .as_ref()
            .and_then(|d| fs::read_to_string(d.join("device")).ok())
            .map(|s| s.trim().trim_start_matches("0x").to_string())
            .unwrap_or_default();
        let driver = dev_dir
            .as_ref()
            .and_then(|d| fs::read_link(d.join("driver")).ok())
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let mut name = String::from("Intel GPU");
        if !id.is_empty() {
            name.push_str(&format!(" 8086:{id}"));
        }
        if !driver.is_empty() {
            name.push_str(&format!(" ({driver})"));
        }
        if let Some(n) = &node {
            name.push_str(&format!(" · {n}"));
        }
        IntelSampler { pdev, card, name, prev: HashMap::new(), prev_t: Instant::now() }
    }

    fn freq(&self) -> (Option<u32>, Option<u32>) {
        let Some(c) = &self.card else { return (None, None) };
        let i915 = (read_u32(&c.join("gt_cur_freq_mhz")), read_u32(&c.join("gt_max_freq_mhz")));
        if i915.0.is_some() {
            return i915;
        }
        let xe = c.join("device/tile0/gt0/freq0");
        (read_u32(&xe.join("cur_freq")), read_u32(&xe.join("max_freq")))
    }

    /// Klienci DRM karty Intel otwarci przez podane procesy (klucz: pdev/client-id).
    fn read_clients(&self, pids: &[u32]) -> HashMap<String, Client> {
        let mut out: HashMap<String, Client> = HashMap::new();
        for pid in pids {
            let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
            for fd in fds.filter_map(|e| e.ok()) {
                let is_drm = fs::read_link(fd.path())
                    .map(|t| t.to_string_lossy().starts_with("/dev/dri/"))
                    .unwrap_or(false);
                if !is_drm {
                    continue;
                }
                let fdn = fd.file_name().to_string_lossy().to_string();
                let Ok(info) = fs::read_to_string(format!("/proc/{pid}/fdinfo/{fdn}")) else { continue };
                let kv: Vec<(&str, &str)> = info
                    .lines()
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim(), v.trim()))
                    .collect();
                let get = |key: &str| kv.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
                if !matches!(get("drm-driver"), Some("i915") | Some("xe")) {
                    continue;
                }
                let pdev = get("drm-pdev").unwrap_or("");
                if let Some(want) = &self.pdev {
                    if pdev != want {
                        continue;
                    }
                }
                let key = format!("{pdev}/{}", get("drm-client-id").unwrap_or(&fdn));
                if out.contains_key(&key) {
                    continue; // ten sam klient przez kilka deskryptorów
                }
                let mut c = Client::default();
                for (k, v) in &kv {
                    if let Some(cls) = k.strip_prefix("drm-engine-capacity-") {
                        if let (Some(cl), Some(n)) = (class_of(cls), parse_value(v)) {
                            *c.cap.entry(cl).or_default() += n;
                        }
                    } else if let Some(cls) = k.strip_prefix("drm-engine-") {
                        if let (Some(cl), Some(n)) = (class_of(cls), parse_value(v)) {
                            *c.busy.entry(cl).or_default() += n;
                        }
                    } else if let Some(cls) = k.strip_prefix("drm-total-cycles-") {
                        if let (Some(cl), Some(n)) = (class_of(cls), parse_value(v)) {
                            let e = c.total.entry(cl).or_default();
                            *e = (*e).max(n);
                        }
                    } else if let Some(cls) = k.strip_prefix("drm-cycles-") {
                        if let (Some(cl), Some(n)) = (class_of(cls), parse_value(v)) {
                            *c.busy.entry(cl).or_default() += n;
                        }
                    } else if k.starts_with("drm-resident-") {
                        c.mem += parse_value(v).unwrap_or(0);
                    }
                }
                out.insert(key, c);
            }
        }
        out
    }

    /// Wołać ~1×/s z PID-ami aktywnych procesów ffmpeg.
    pub fn sample(&mut self, pids: &[u32]) -> IntelStats {
        let now = Instant::now();
        let dt_ns = now.duration_since(self.prev_t).as_nanos() as f64;
        let clients = self.read_clients(pids);

        let mut busy: HashMap<&'static str, f64> = HashMap::new();
        let mut total: HashMap<&'static str, f64> = HashMap::new();
        let mut cap: HashMap<&'static str, f64> = HashMap::new();
        let mut have_delta = false;
        for (key, c) in &clients {
            let Some(p) = self.prev.get(key) else { continue };
            have_delta = true;
            for (cls, b) in &c.busy {
                let d = b.saturating_sub(*p.busy.get(cls).unwrap_or(b));
                *busy.entry(*cls).or_default() += d as f64;
                let k = c.cap.get(cls).copied().unwrap_or(1).max(1) as f64;
                let e = cap.entry(*cls).or_insert(k);
                *e = e.max(k);
                if let (Some(t), Some(pt)) = (c.total.get(cls), p.total.get(cls)) {
                    let e = total.entry(*cls).or_default();
                    *e = e.max(t.saturating_sub(*pt) as f64);
                }
            }
        }

        let util = |cls: &str| -> Option<f64> {
            if clients.is_empty() {
                return Some(0.0); // żaden nasz proces nie używa karty (np. analiza na CPU)
            }
            if !have_delta {
                return None; // pierwszy odczyt nowego procesu
            }
            let b = busy.get(cls).copied().unwrap_or(0.0);
            let k = cap.get(cls).copied().unwrap_or(1.0);
            let base = match total.get(cls) {
                Some(t) if *t > 0.0 => *t, // xe: cykle odniesienia
                _ => dt_ns,                // i915: czas ścienny w ns
            };
            (base > 0.0).then(|| (100.0 * b / (base * k)).clamp(0.0, 100.0))
        };

        let (freq_mhz, freq_max_mhz) = self.freq();
        let mem: u64 = clients.values().map(|c| c.mem).sum();
        let stats = IntelStats {
            name: self.name.clone(),
            video: util("video"),
            enhance: util("enhance"),
            render: util("render"),
            freq_mhz,
            freq_max_mhz,
            mem_bytes: (!clients.is_empty()).then_some(mem),
        };
        self.prev = clients;
        self.prev_t = now;
        stats
    }
}
