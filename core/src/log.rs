//! Bufor logu (TUI) albo echo na stdout (tryb skryptowy — bez buforowania w RAM).
use std::{
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ECHO: AtomicBool = AtomicBool::new(false);
const MAX_LINES: usize = 500;

/// Tryb skryptowy: linie idą od razu na stdout i nie są przechowywane.
pub fn set_echo(v: bool) {
    ECHO.store(v, Ordering::SeqCst);
}

pub fn push(s: String) {
    if ECHO.load(Ordering::SeqCst) {
        let mut out = std::io::stdout().lock();
        if s.is_empty() {
            let _ = writeln!(out);
        }
        for l in s.lines() {
            let _ = writeln!(out, "{l}");
        }
        return;
    }
    let Ok(mut g) = LOG.lock() else { return };
    if s.is_empty() {
        g.push(String::new());
    }
    for l in s.lines() {
        g.push(l.chars().filter(|c| !c.is_control()).collect());
    }
    if g.len() > MAX_LINES {
        let d = g.len() - MAX_LINES;
        g.drain(0..d);
    }
}

pub fn tail(n: usize) -> Vec<String> {
    let Ok(g) = LOG.lock() else { return vec![] };
    g[g.len().saturating_sub(n)..].to_vec()
}

pub fn clear() {
    if let Ok(mut g) = LOG.lock() {
        g.clear();
    }
}
