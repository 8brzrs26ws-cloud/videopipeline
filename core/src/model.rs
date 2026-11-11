//! Model stanu pipeline'u współdzielony między wątkiem roboczym a TUI.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Waiting,
    Analysis,
    Nvenc,
    NvencDone,
    Hdr,
    Hdr10Plus,
    Dv,
    Audio,
    Mux,
    Done,
    Error,
}

impl State {
    pub fn percent(self) -> f64 {
        match self {
            State::Waiting | State::Error => 0.0,
            State::Analysis => 0.0,
            State::Nvenc => 15.0,
            State::NvencDone | State::Hdr => 75.0,
            State::Hdr10Plus => 78.0,
            State::Dv => 84.0,
            State::Audio => 90.0,
            State::Mux => 97.0,
            State::Done => 100.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            State::Waiting => "OCZEKUJE",
            State::Analysis => "ANALIZA",
            State::Nvenc => "KODOWANIE",
            State::NvencDone => "ZAKODOWANE",
            State::Hdr => "HDR",
            State::Hdr10Plus => "HDR10PLUS",
            State::Dv => "DV",
            State::Audio => "AUDIO",
            State::Mux => "MUX",
            State::Done => "GOTOWE",
            State::Error => "BŁĄD",
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub state: State,
    pub note: String,
    pub progress_file: Option<PathBuf>,
    pub duration_us: Option<f64>,
    /// Historia fps do wykresu.
    pub history: Vec<f64>,
    /// Postęp analizy pliku 0–100 (m.in. odczyt strumienia dla Dolby Vision / HDR10+).
    pub analysis_pct: Option<f64>,
}

#[derive(Clone, Default)]
pub struct AnalysisView {
    pub name: String,
    pub pct: f64,
    pub step: String,
    pub note: String,
}

#[derive(Default)]
struct Inner {
    items: BTreeMap<String, Item>,
    title: String,
    analysis: Option<AnalysisView>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub items: BTreeMap<String, Item>,
    pub title: String,
    pub analysis: Option<AnalysisView>,
}

/// Tani w klonowaniu uchwyt do współdzielonego stanu — wszystkie metody biorą &self.
#[derive(Clone, Default)]
pub struct Panel(Arc<Mutex<Inner>>);

#[derive(Default)]
pub struct FfProgress {
    pub out_us: Option<f64>,
    pub speed: Option<String>,
    pub fps: Option<String>,
    pub ended: bool,
}

/// ffmpeg -progress dopisuje rekordy; bierzemy ostatnie poprawne wartości.
pub fn read_progress(path: &Path) -> FfProgress {
    let mut p = FfProgress::default();
    let Ok(text) = fs::read_to_string(path) else { return p };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            match k {
                "out_time_us" => {
                    if let Ok(x) = v.parse::<f64>() {
                        p.out_us = Some(x);
                    }
                }
                "speed" => p.speed = Some(v.to_string()),
                "fps" => p.fps = Some(v.to_string()),
                "progress" => p.ended = v == "end",
                _ => {}
            }
        }
    }
    p
}

pub fn hms(sec: f64) -> String {
    let t = sec.max(0.0).floor() as u64;
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

pub struct Enc {
    pub pct: f64,
    pub cur: f64,
    pub dur: f64,
    pub prog: FfProgress,
}

impl Enc {
    /// Szacowany czas do końca na podstawie bieżącego `speed`.
    pub fn eta(&self) -> String {
        let speed = self.prog.speed.clone().unwrap_or_default();
        let snum = speed.trim_end_matches('x').parse::<f64>().unwrap_or(0.0);
        if snum > 0.0 && self.dur > self.cur {
            hms((self.dur - self.cur) / snum)
        } else {
            "--:--:--".into()
        }
    }
}

pub fn enc_progress(it: &Item) -> Option<Enc> {
    let prog = read_progress(it.progress_file.as_ref()?);
    let out = prog.out_us?;
    let total = it.duration_us?;
    if total <= 0.0 {
        return None;
    }
    Some(Enc { pct: 100.0 * out / total, cur: out / 1e6, dur: total / 1e6, prog })
}

/// Postęp całego pipeline'u dla pliku: analiza 0–15%, kodowanie 15–75%, dalej etapy HDR/audio/mux.
pub fn overall_pct(it: &Item, enc: &Option<Enc>) -> f64 {
    match (it.state, enc) {
        (State::Done, _) => 100.0,
        (State::Analysis, _) => it.analysis_pct.unwrap_or(0.0).clamp(0.0, 100.0) * 0.15,
        (State::Nvenc, Some(e)) => 15.0 + e.pct.clamp(0.0, 100.0) * 0.60,
        _ => it.state.percent(),
    }
}

impl Panel {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn register(&self, name: &str) {
        self.lock().items.entry(name.to_string()).or_insert(Item {
            state: State::Waiting,
            note: String::new(),
            progress_file: None,
            duration_us: None,
            history: Vec::new(),
            analysis_pct: None,
        });
    }

    pub fn set(&self, name: &str, state: State, note: impl Into<String>) {
        self.register(name);
        if let Some(it) = self.lock().items.get_mut(name) {
            it.state = state;
            it.note = note.into();
        }
    }

    pub fn set_progress(&self, name: &str, file: PathBuf, duration_us: Option<f64>) {
        self.register(name);
        if let Some(it) = self.lock().items.get_mut(name) {
            it.progress_file = Some(file);
            it.duration_us = duration_us;
            it.history.clear();
        }
    }

    /// Ustawia tytuł etapu (wołane przez kod pipeline'u).
    pub fn draw(&self, title: &str) {
        let mut g = self.lock();
        g.title = title.trim().to_string();
        g.analysis = None;
    }

    pub fn draw_analysis(&self, name: &str, pct: f64, step: &str, note: &str) {
        let mut g = self.lock();
        // Pasek pliku w kolejce i ALL: postęp analizy (w tym odczytu strumienia dla DV / HDR10+).
        if let Some(it) = g.items.get_mut(name).filter(|it| it.state == State::Analysis) {
            it.analysis_pct = Some(pct);
            it.note = step.to_string();
        }
        g.title = "ETAP 1/3 — ANALIZA PLIKÓW".into();
        g.analysis = Some(AnalysisView {
            name: name.into(),
            pct,
            step: step.into(),
            note: note.into(),
        });
    }

    /// Dopisuje próbkę fps do historii aktywnych kodowań (wołane ~1x/s przez TUI).
    pub fn tick(&self) {
        let mut g = self.lock();
        for it in g.items.values_mut() {
            if it.state == State::Nvenc {
                if let Some(e) = enc_progress(it) {
                    if let Some(f) = e.prog.fps.as_deref().and_then(|x| x.parse::<f64>().ok()) {
                        it.history.push(f);
                        if it.history.len() > 400 {
                            it.history.remove(0);
                        }
                    }
                }
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        Snapshot { items: g.items.clone(), title: g.title.clone(), analysis: g.analysis.clone() }
    }
}
