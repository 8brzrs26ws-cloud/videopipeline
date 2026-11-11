//! Tryb skryptowy: bez TUI, bez ratatui — tekstowy postęp i kod wyjścia dla skryptów.
use anyhow::Result;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use videopipeline_core::{
    config::*,
    files, log,
    model::{enc_progress, Panel, State},
    pipeline::{self, RunStatus, SharedStatus},
    proc,
};

fn summary(o: &Opts) -> String {
    let label = o.codec_label();
    let cq = format!("CQ {}", o.cq());
    let mut p: Vec<&str> = vec![label.as_str(), cq.as_str()];
    if o.pl_eac3 {
        p.push("PL E-AC-3 192k");
    }
    if o.extra {
        p.push("+EN +napisy");
    }
    if o.denoise {
        p.push("odszumianie");
    }
    if o.no_crop {
        p.push("bez crop");
    }
    if o.no_dv {
        p.push("bez DV");
    }
    if o.no_hdr10plus {
        p.push("bez HDR10+");
    }
    if o.legacy_inject {
        p.push("DV/HDR10+ przez pliki tymczasowe");
    }
    if o.scale_1080 {
        p.push("skalowanie do 1080p");
    }
    if o.copy {
        p.push("kopia → encoded");
    }
    p.join(" · ")
}

/// Zwraca kod wyjścia: 0 = OK, 1 = błędy, 130 = przerwano.
pub fn run(opts: Opts, paths: Vec<PathBuf>) -> Result<i32> {
    log::set_echo(true); // log idzie od razu na stdout, bez buforowania
    let list = files::discover(&paths)?;
    if list.is_empty() {
        println!("Brak plików wideo do przetworzenia. Zakończenie.");
        return Ok(0);
    }
    println!(
        "VideoPipeline {} — tryb skryptowy: {} plik(ów) — {}",
        env!("CARGO_PKG_VERSION"),
        list.len(),
        summary(&opts)
    );

    ctrlc::set_handler(|| {
        eprintln!("\nPrzerwanie — zatrzymuję aktywne procesy ffmpeg...");
        proc::set_cancel(true);
        proc::kill_all();
    })?;

    let panel = Panel::default();
    let status: SharedStatus = Arc::new(Mutex::new(RunStatus::Running));
    let handle = pipeline::spawn(opts.clone(), list, panel.clone(), status.clone());

    let eng = opts.engine.label();
    let mut last: HashMap<String, State> = HashMap::new();
    let mut last_progress = Instant::now();
    loop {
        // Status czytamy przed migawką, żeby ostatnia migawka zawierała stany końcowe.
        let finished = status.lock().map(|g| *g != RunStatus::Running).unwrap_or(true);
        let snap = panel.snapshot();

        for (name, it) in &snap.items {
            if last.get(name) != Some(&it.state) {
                if it.state != State::Waiting {
                    let note = if it.note.is_empty() { String::new() } else { format!(" — {}", it.note) };
                    println!("[{name}] {}{note}", it.state.label());
                }
                last.insert(name.clone(), it.state);
            }
        }
        if last_progress.elapsed() >= Duration::from_secs(10) {
            last_progress = Instant::now();
            for (name, it) in &snap.items {
                if it.state != State::Nvenc {
                    continue;
                }
                if let Some(e) = enc_progress(it) {
                    println!(
                        "    [{name}] {eng} {:.1}% · {} fps · {} · ETA {}",
                        e.pct,
                        e.prog.fps.clone().unwrap_or_else(|| "--".into()),
                        e.prog.speed.clone().unwrap_or_else(|| "--".into()),
                        e.eta()
                    );
                }
            }
        }
        if finished {
            break;
        }
        thread::sleep(Duration::from_secs(1));
    }
    let _ = handle.join();

    let st = status.lock().map(|g| g.clone()).unwrap_or(RunStatus::Done);
    let snap = panel.snapshot();
    let done = snap.items.values().filter(|i| i.state == State::Done).count();
    let errors: Vec<_> = snap.items.iter().filter(|(_, i)| i.state == State::Error).collect();
    println!("\nPodsumowanie: gotowe {done}/{}, błędy {}", snap.items.len(), errors.len());
    for (name, it) in &errors {
        println!("  BŁĄD: {name} — {}", it.note);
    }

    match st {
        RunStatus::Cancelled => {
            println!("Przerwano.");
            return Ok(130);
        }
        RunStatus::Failed(msg) => {
            eprintln!("BŁĄD: {msg}");
            return Ok(1);
        }
        _ => {}
    }
    println!("ZAKOŃCZONO. Skompresowane pliki w: {}", opts.output_dir.display());

    if opts.shutdown {
        println!("Wyłączanie komputera za 10 s — Ctrl+C anuluje.");
        for _ in 0..10 {
            if proc::cancelled() {
                return Ok(130);
            }
            thread::sleep(Duration::from_secs(1));
        }
        if !proc::cancelled() {
            videopipeline_core::platform::shutdown_now();
        }
    }
    Ok(if errors.is_empty() { 0 } else { 1 })
}
