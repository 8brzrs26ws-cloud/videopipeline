//! Binarka: tylko interfejs. Cała logika jest w bibliotece dynamicznej videopipeline_core.
mod app;
mod browser;
mod cli;
mod perm;
mod player;
mod render;
mod term;
mod theme;
mod window;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use videopipeline_core::{
    config::*,
    files,
    hw::{self, HwInfo},
    platform,
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
    /// Preset QSV: veryfast, faster, fast, medium, slow, slower, veryslow (domyślnie slow)
    #[arg(long = "qsv-preset", value_parser = QSV_PRESETS)]
    qsv_preset: Option<String>,
    /// Pokaż wykryte enkodery GPU i zakończ
    #[arg(long)]
    detect: bool,
    /// Wypisz pełne informacje MediaInfo o podanych plikach (katalog = wszystkie filmy w nim) i zakończ
    #[arg(long = "mediainfo")]
    mediainfo: bool,
    /// KDE Plasma: reguła okna KWin z ikoną VideoPipeline na pasku zadań (domyślnie dodawana przy starcie TUI)
    #[arg(long = "kde-icon", value_name = "on|off", value_parser = ["on", "off"])]
    kde_icon: Option<String>,
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
    if !platform::has_exe(dir, "ffmpeg") {
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

/// `--mediainfo`: pełny raport MediaInfo dla każdego pliku (jak `mediainfo plik`).
fn print_mediainfo(paths: &[PathBuf]) -> Result<i32> {
    if paths.is_empty() {
        eprintln!("Podaj plik lub katalog: videopipeline --mediainfo film.mkv");
        return Ok(1);
    }
    let mut files = vec![];
    for p in paths {
        if p.is_dir() {
            files.extend(files::videos_in(p));
        } else {
            files.push(p.clone());
        }
    }
    let mut code = 0;
    for (i, f) in files.iter().enumerate() {
        if i > 0 {
            println!();
        }
        match videopipeline_core::mediainfo::full(f) {
            Ok(t) => print!("{t}"),
            Err(e) => {
                eprintln!("{}: {e}", f.display());
                code = 1;
            }
        }
    }
    term::wait_for_enter_if_relaunched();
    Ok(code)
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

/// Wydania są kompilowane pod x86-64-v3 (AVX2, BMI2, FMA). Na starszym procesorze program
/// zakończyłby się błędem „illegal instruction” — lepiej powiedzieć to wprost.
/// (Linux AppImage sprawdza to też w AppRun; ten test działa na każdej platformie.)
fn cpu_ok() -> bool {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    {
        std::is_x86_feature_detected!("avx2")
            && std::is_x86_feature_detected!("bmi2")
            && std::is_x86_feature_detected!("fma")
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
    {
        true
    }
}

fn real_main() -> Result<i32> {
    if !cpu_ok() {
        eprintln!("Ta wersja VideoPipeline wymaga procesora x86-64-v3 (AVX2, BMI2, FMA):");
        eprintln!("Intel Haswell (Core 4. gen.) / AMD Zen 1 lub nowszego.");
        term::wait_for_enter_if_relaunched();
        return Ok(1);
    }
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
        "-mi" => "--mediainfo".to_string(),
        "-nvenc" => "--engine=nvenc".to_string(),
        "-qsv" => "--engine=qsv".to_string(),
        "-vaapi" => "--engine=vaapi".to_string(),
        _ => a,
    });
    let cli = Cli::parse_from(args);
    let codec = if cli.av1 { Codec::Av1 } else { Codec::Hevc };
    let script = !cli.tui && (!cli.paths.is_empty() || cli.batch);

    if cli.mediainfo {
        return print_mediainfo(&cli.paths);
    }

    if let Some(v) = &cli.kde_icon {
        #[cfg(unix)]
        {
            return match window::kde::set_enabled(v == "on") {
                Ok(m) => {
                    println!("{m}");
                    Ok(0)
                }
                Err(e) => anyhow::bail!("{e}"),
            };
        }
        #[cfg(not(unix))]
        {
            let _ = v;
            println!("--kde-icon dotyczy tylko KDE Plasma (Linux).");
            return Ok(0);
        }
    }

    if cli.detect {
        // Diagnostyka: wersja ffmpeg, każdy test z poleceniem i błędem ffmpeg.
        match hw::ffmpeg_version() {
            Some(v) => println!("{v}"),
            None => println!("ffmpeg: nie znaleziono albo nie uruchamia się"),
        }
        let (hw, reports, qsv_tries) = hw::detect_report();
        for (dev, ok, cmd, err) in &qsv_tries {
            println!("\n[{}] urządzenie QSV: {dev}", if *ok { "OK  " } else { "BRAK" });
            if !ok {
                println!("  polecenie: {cmd}");
                for l in err.lines().filter(|l| !l.trim().is_empty()).take(8) {
                    println!("  {l}");
                }
            }
        }
        for r in &reports {
            let mark = if r.ok { "OK  " } else { "BRAK" };
            println!("\n[{mark}] {} {}", r.codec.label(), r.engine.label());
            if !r.ok {
                if !r.command.is_empty() {
                    println!("  polecenie: {}", r.command);
                }
                for l in r.error.lines().filter(|l| !l.trim().is_empty()).take(8) {
                    println!("  {l}");
                }
            }
        }
        println!("\nWykryte enkodery: {}", hw.summary());
        if let Some(d) = &hw.intel_dev {
            println!("Karta Intel: {}", d.display());
        }
        term::wait_for_enter_if_relaunched();
        return Ok(0);
    }

    // TUI uruchomione dwuklikiem nie ma terminala — najpierw własne okno, dopiero potem wykrywanie.
    // Windows: pliki przeciągnięte na videopipeline.exe uruchamiają tryb skryptowy we własnym oknie
    // konsoli — oznaczamy je, żeby okno nie zniknęło przed przeczytaniem wyniku.
    if !script || cfg!(windows) {
        if let Some(code) = term::relaunch_in_terminal() {
            return Ok(code);
        }
    }
    eprintln!("Wykrywanie enkoderów GPU…");
    let hw = hw::detect();
    let picked = pick_engine(&cli, &hw, codec);

    let opts = Opts {
        codec,
        engine: *picked.as_ref().unwrap_or(&Engine::Nvenc),
        hevc_cq: cli.hevc_cq.or(if cli.av1 { None } else { cli.cq }).unwrap_or(DEFAULT_HEVC_CQ),
        av1_cq: cli.av1_cq.or(if cli.av1 { cli.cq } else { None }).unwrap_or(DEFAULT_AV1_CQ),
        qsv_preset: cli
            .qsv_preset
            .as_deref()
            .and_then(|p| QSV_PRESETS.iter().copied().find(|x| *x == p))
            .unwrap_or(DEFAULT_QSV_PRESET),
        pl_eac3: cli.pleac3,
        extra: cli.da,
        copy: cli.copy,
        denoise: cli.denoise,
        no_crop: cli.no_crop,
        no_dv: cli.no_dv,
        no_hdr10plus: cli.no_hdr10plus,
        shutdown: cli.shutdown,
        output_dir: platform::default_output_dir(),
    };

    // Tryb skryptowy: żadnego ratatui, tylko zwykły tekst — mniej RAM-u.
    if script {
        println!("Wykryte enkodery: {}", hw.summary());
        let opts = match picked {
            Ok(_) => opts,
            Err(e) => anyhow::bail!("{e}"),
        };
        let paths = if cli.paths.is_empty() { vec![PathBuf::from(".")] } else { cli.paths };
        let code = cli::run(opts, paths)?;
        if cfg!(windows) {
            term::wait_for_enter_if_relaunched(); // okno konsoli z Eksploratora
        }
        return Ok(code);
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

    #[cfg(unix)]
    window::kde::auto_install(); // KDE: ikona VideoPipeline zamiast Konsole na pasku zadań

    let mut app = app::App::new(opts, hw, start_dir, initial);
    if let Err(e) = picked {
        app.status_msg = format!("{e} Wybierz kodek w F7.");
    }
    app::run(&mut app)?;

    if app.do_shutdown {
        platform::shutdown_now();
    }
    Ok(0)
}
