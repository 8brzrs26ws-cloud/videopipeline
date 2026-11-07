//! Binarka: tylko interfejs. Cała logika jest w bibliotece dynamicznej videopipeline_core.
mod app;
mod browser;
mod cli;
mod render;
mod term;
mod theme;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use videopipeline_core::{
    config::*,
    files,
    hw::{self, HwInfo},
};

#[derive(Parser)]
#[command(
    name = "videopipeline",
    version,
    about = "VideoPipeline — TUI w stylu Midnight Commandera; z podanymi plikami/katalogami koduje od razu (tryb skryptowy)"
)]
struct Cli {
    /// Pliki lub katalogi do zakodowania. Gdy podane: tryb skryptowy (bez TUI), jak w V9
    paths: Vec<PathBuf>,
    /// Wyłącz komputer po zakończeniu (w TUI: wartość początkowa opcji)
    #[arg(short = 's', long)]
    shutdown: bool,
    /// Łagodny denoiser dla SDR 8-bit (CUDA, fallback CPU)
    #[arg(short = 'd', long)]
    denoise: bool,
    /// Skopiuj wynik do source/encoded i zweryfikuj SHA-512
    #[arg(short = 'c', long)]
    copy: bool,
    /// Koduj do AV1 zamiast HEVC
    #[arg(long)]
    av1: bool,
    /// Koduj na karcie Intel (QSV, zapasowo VAAPI). Bez NVIDII w systemie Intel jest wybierany sam
    #[arg(long)]
    intel: bool,
    /// Wymuś silnik: nvenc, qsv albo vaapi (domyślnie NVIDIA NVENC, gdy jest dostępna)
    #[arg(long, value_parser = ["nvenc", "qsv", "vaapi"])]
    engine: Option<String>,
    /// Pokaż wykryte enkodery GPU i zakończ
    #[arg(long)]
    detect: bool,
    /// Pierwsza polska ścieżka -> E-AC-3 stereo 192 kb/s
    #[arg(long)]
    pleac3: bool,
    /// Dodatkowo audio EN (E-AC-3 5.1) oraz napisy EN i PL
    #[arg(long = "da")]
    da: bool,
    /// Jakość HEVC — CQ (mniej = lepiej; domyślnie 19)
    #[arg(long = "hevc-cq", value_parser = clap::value_parser!(u8).range(1..=51))]
    hevc_cq: Option<u8>,
    /// Jakość AV1 — CQ (mniej = lepiej; domyślnie 25)
    #[arg(long = "av1-cq", value_parser = clap::value_parser!(u8).range(1..=51))]
    av1_cq: Option<u8>,
    /// Jakość dla wybranego kodeka (skrót: ustawia --hevc-cq albo --av1-cq zależnie od --av1)
    #[arg(short = 'q', long = "cq", value_parser = clap::value_parser!(u8).range(1..=51))]
    cq: Option<u8>,
    /// Pomiń cropdetect (-pc)
    #[arg(long = "no-crop")]
    no_crop: bool,
    /// Pomiń analizę Dolby Vision (-pdv)
    #[arg(long = "no-dv")]
    no_dv: bool,
    /// Pomiń analizę HDR10+ (-ph)
    #[arg(long = "no-hdr10plus")]
    no_hdr10plus: bool,
    /// Tryb skryptowy także bez ścieżek (wtedy katalog bieżący, jak w V9)
    #[arg(short = 'b', long)]
    batch: bool,
    /// Wymuś TUI nawet gdy podano ścieżki (trafią do kolejki)
    #[arg(long)]
    tui: bool,
}

fn main() {
    match real_main() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("BŁĄD: {e:#}");
            term::wait_for_enter_if_relaunched();
            std::process::exit(1);
        }
    }
}

/// Pakiety (deb w /opt, AppImage) trzymają ffmpeg, mkvmerge itd. obok binarki — dopisujemy ten katalog
/// do PATH, żeby narzędzia były znalezione także po ponownym uruchomieniu w terminalu.
fn prepend_bundled_tools_to_path() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    if !dir.join("ffmpeg").is_file() {
        return;
    }
    let mut paths = vec![dir.to_path_buf()];
    if let Some(old) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&old));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

/// Silnik: --engine > --intel (QSV, zapasowo VAAPI) > domyślny (NVIDIA, a bez niej Intel).
fn pick_engine(cli: &Cli, hw: &HwInfo, codec: Codec) -> std::result::Result<Engine, String> {
    if let Some(e) = cli.engine.as_deref().and_then(Engine::from_name) {
        return if hw.has(e, codec) {
            Ok(e)
        } else {
            Err(format!(
                "{} {} nie działa w tym systemie (wykryte: {}).",
                codec.label(),
                e.label(),
                hw.summary()
            ))
        };
    }
    if cli.intel {
        return hw.intel_engine(codec).ok_or_else(|| {
            format!("Brak działającego enkodera Intel dla {} (wykryte: {}).", codec.label(), hw.summary())
        });
    }
    hw.default_engine(codec)
}

fn real_main() -> Result<i32> {
    prepend_bundled_tools_to_path();
    // Zachowujemy składnię V9: -av1 / -pleac3 / -da (+ nowe -nocrop / -nodv / -nohdr10plus).
    let args = std::env::args().map(|a| match a.as_str() {
        "-av1" => "--av1".to_string(),
        "-pleac3" => "--pleac3".to_string(),
        "-da" => "--da".to_string(),
        "-pc" | "-nocrop" => "--no-crop".to_string(),
        "-pdv" | "-nodv" => "--no-dv".to_string(),
        "-ph" | "-nohdr10plus" => "--no-hdr10plus".to_string(),
        "-intel" => "--intel".to_string(),
        "-nvenc" => "--engine=nvenc".to_string(),
        "-qsv" => "--engine=qsv".to_string(),
        "-vaapi" => "--engine=vaapi".to_string(),
        _ => a,
    });
    let cli = Cli::parse_from(args);
    let codec = if cli.av1 { Codec::Av1 } else { Codec::Hevc };
    let script = !cli.tui && (!cli.paths.is_empty() || cli.batch);

    if cli.detect {
        let hw = hw::detect();
        println!("Wykryte enkodery: {}", hw.summary());
        if let Some(d) = &hw.intel_dev {
            println!("Karta Intel: {}", d.display());
        }
        return Ok(0);
    }

    // TUI uruchomione dwuklikiem nie ma terminala — najpierw własne okno, dopiero potem wykrywanie.
    if !script {
        if let Some(code) = term::relaunch_in_terminal() {
            return Ok(code);
        }
    }
    eprintln!("Wykrywanie enkoderów GPU…");
    let hw = hw::detect();
    let picked = pick_engine(&cli, &hw, codec);

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let opts = Opts {
        codec,
        engine: *picked.as_ref().unwrap_or(&Engine::Nvenc),
        hevc_cq: cli.hevc_cq.or(if cli.av1 { None } else { cli.cq }).unwrap_or(DEFAULT_HEVC_CQ),
        av1_cq: cli.av1_cq.or(if cli.av1 { cli.cq } else { None }).unwrap_or(DEFAULT_AV1_CQ),
        pl_eac3: cli.pleac3,
        extra: cli.da,
        copy: cli.copy,
        denoise: cli.denoise,
        no_crop: cli.no_crop,
        no_dv: cli.no_dv,
        no_hdr10plus: cli.no_hdr10plus,
        shutdown: cli.shutdown,
        output_dir: PathBuf::from(home).join("Wideo"),
    };

    // Tryb skryptowy: żadnego ratatui, tylko zwykły tekst — mniej RAM-u.
    if script {
        println!("Wykryte enkodery: {}", hw.summary());
        let opts = match picked {
            Ok(_) => opts,
            Err(e) => anyhow::bail!("{e}"),
        };
        let paths = if cli.paths.is_empty() { vec![PathBuf::from(".")] } else { cli.paths };
        return cli::run(opts, paths);
    }

    let _pause = term::PauseOnExit::new(); // gdy okno terminala było nasze: pokaż błąd przed zamknięciem

    let (start_dir, initial) = if cli.paths.is_empty() {
        (std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")), vec![])
    } else {
        let initial = files::discover(&cli.paths)?;
        let first = cli.paths[0].canonicalize().unwrap_or_else(|_| cli.paths[0].clone());
        let dir = if first.is_dir() {
            first
        } else {
            first.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
        };
        (dir, initial)
    };

    let mut app = app::App::new(opts, hw, start_dir, initial);
    if let Err(e) = picked {
        app.status_msg = format!("{e} Wybierz kodek w F7.");
    }
    app::run(&mut app)?;

    if app.do_shutdown {
        let _ = std::process::Command::new("shutdown").arg("now").status();
    }
    Ok(0)
}
