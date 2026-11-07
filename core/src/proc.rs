//! Procesy potomne: rejestr PID-ów (do przerwania) i flaga anulowania.
use std::{
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::Duration,
};

pub static PIDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

pub fn set_cancel(v: bool) {
    CANCEL.store(v, Ordering::SeqCst);
}

pub fn register(pid: u32) {
    if let Ok(mut g) = PIDS.lock() {
        g.push(pid);
    }
}

pub fn unregister(pid: u32) {
    if let Ok(mut g) = PIDS.lock() {
        g.retain(|p| *p != pid);
    }
}

/// Zatrzymuje wszystkie zarejestrowane procesy (TERM, potem KILL).
pub fn kill_all() {
    let pids: Vec<u32> = PIDS.lock().map(|g| g.clone()).unwrap_or_default();
    for force in [false, true] {
        for p in &pids {
            crate::platform::kill_pid(*p, force);
        }
        if !force {
            thread::sleep(Duration::from_millis(500));
        }
    }
}

/// Uruchamia polecenie (stdout/stderr -> null), zwraca kod wyjścia.
/// stdin ustawia wołający.
pub fn run(cmd: &mut Command) -> Option<i32> {
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let id = child.id();
    register(id);
    let st = child.wait();
    unregister(id);
    st.ok()?.code()
}

/// Uruchamia polecenie i zbiera wyjście (stdin -> null).
pub fn output(cmd: &mut Command) -> Option<Output> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().ok()?;
    let id = child.id();
    register(id);
    let out = child.wait_with_output().ok();
    unregister(id);
    out
}
