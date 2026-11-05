//! Monitoring obciążenia: GPU/NVENC/NVDEC/VRAM (nvidia-smi w pętli), CPU i RAM (/proc).
use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Default, Debug)]
pub struct GpuStats {
    pub name: String,
    pub gpu: Option<f64>,
    pub enc: Option<f64>,
    pub dec: Option<f64>,
    pub vram_used_mib: Option<f64>,
    pub vram_total_mib: Option<f64>,
}

#[derive(Clone, Default, Debug)]
pub struct SysStats {
    pub gpu: Option<GpuStats>,
    /// Użycie CPU w % (cały system).
    pub cpu: Option<f64>,
    pub ram_used: Option<u64>,
    pub ram_total: Option<u64>,
}

// Starsze sterowniki mogą nie znać utilization.encoder/decoder — wtedy zestaw podstawowy.
const FIELDS_FULL: &[&str] = &[
    "index",
    "utilization.gpu",
    "utilization.encoder",
    "utilization.decoder",
    "memory.used",
    "memory.total",
    "name",
];
const FIELDS_BASIC: &[&str] = &["index", "utilization.gpu", "memory.used", "memory.total", "name"];

fn parse_line(fields: &[&str], line: &str) -> Option<GpuStats> {
    let parts: Vec<&str> = line.splitn(fields.len(), ',').map(|s| s.trim()).collect();
    if parts.len() != fields.len() {
        return None;
    }
    let num = |v: &str| v.parse::<f64>().ok(); // "[N/A]" -> None
    let mut g = GpuStats::default();
    let mut index = None;
    for (f, v) in fields.iter().zip(parts.iter()) {
        match *f {
            "index" => index = v.parse::<u32>().ok(),
            "utilization.gpu" => g.gpu = num(v),
            "utilization.encoder" => g.enc = num(v),
            "utilization.decoder" => g.dec = num(v),
            "memory.used" => g.vram_used_mib = num(v),
            "memory.total" => g.vram_total_mib = num(v),
            "name" => g.name = v.to_string(),
            _ => {}
        }
    }
    // NVENC/CUDA domyślnie używają GPU 0.
    (index == Some(0)).then_some(g)
}

fn gpu_thread(slot: Arc<Mutex<Option<GpuStats>>>, pid: Arc<Mutex<Option<u32>>>, stop: Arc<AtomicBool>) {
    for fields in [FIELDS_FULL, FIELDS_BASIC] {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut child) = Command::new("nvidia-smi")
            .arg(format!("--query-gpu={}", fields.join(",")))
            .args(["--format=csv,noheader,nounits", "-l", "1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return; // brak nvidia-smi
        };
        if let Ok(mut g) = pid.lock() {
            *g = Some(child.id());
        }
        if stop.load(Ordering::SeqCst) {
            // Monitor został zamknięty w trakcie startu nvidia-smi.
            let _ = child.kill();
        }
        let mut got = false;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(g) = parse_line(fields, &line) {
                    got = true;
                    if let Ok(mut s) = slot.lock() {
                        *s = Some(g);
                    }
                }
            }
        }
        let _ = child.wait();
        if let Ok(mut g) = pid.lock() {
            *g = None;
        }
        if got || stop.load(Ordering::SeqCst) {
            return;
        }
    }
}

pub struct Monitor {
    gpu: Arc<Mutex<Option<GpuStats>>>,
    pid: Arc<Mutex<Option<u32>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    prev_cpu: Option<(u64, u64)>, // (total, idle)
}

impl Monitor {
    /// Uruchamia nvidia-smi w pętli (jeden proces na cały czas pracy, nie na każdy odczyt).
    pub fn start() -> Self {
        let gpu = Arc::new(Mutex::new(None));
        let pid = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (g, p, s) = (gpu.clone(), pid.clone(), stop.clone());
            Some(thread::spawn(move || gpu_thread(g, p, s)))
        };
        Monitor { gpu, pid, stop, thread, prev_cpu: read_cpu() }
    }

    /// Wołać ~1x/s. CPU liczone z różnicy odczytów /proc/stat.
    pub fn sample(&mut self) -> SysStats {
        let mut s = SysStats::default();
        s.gpu = self.gpu.lock().ok().and_then(|g| g.clone());

        if let Some((total, idle)) = read_cpu() {
            if let Some((pt, pi)) = self.prev_cpu {
                let dt = total.saturating_sub(pt);
                let di = idle.saturating_sub(pi);
                if dt > 0 {
                    s.cpu = Some((100.0 * (1.0 - di as f64 / dt as f64)).clamp(0.0, 100.0));
                }
            }
            self.prev_cpu = Some((total, idle));
        }
        if let Some((total, avail)) = read_mem() {
            s.ram_total = Some(total);
            s.ram_used = Some(total.saturating_sub(avail));
        }
        s
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(p) = self.pid.lock().ok().and_then(|g| *g) {
            let _ = Command::new("kill").arg("-TERM").arg(p.to_string()).stderr(Stdio::null()).status();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_cpu() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 5 {
        return None;
    }
    let total: u64 = v.iter().take(8).sum();
    Some((total, v[3] + v[4])) // idle + iowait
}

/// (MemTotal, MemAvailable) w bajtach.
fn read_mem() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let get = |key: &str| -> Option<u64> {
        text.lines()
            .find(|l| l.starts_with(key))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    Some((get("MemTotal:")?, get("MemAvailable:")?))
}
