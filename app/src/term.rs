//! Uruchamianie z menedżera plików (np. dwuklik w Dolphinie/KDE): gdy proces nie ma terminala,
//! otwieramy własne okno terminala i uruchamiamy w nim ten sam program z tymi samymi argumentami.
use std::{
    env,
    io::{self, BufRead, IsTerminal, Write},
    process::Command,
};

const MARK: &str = "VIDEOPIPELINE_IN_TERMINAL";

fn has_tty() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn in_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|p| env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// (program, argumenty poprzedzające polecenie). Kolejność: KDE najpierw.
const TERMINALS: &[(&str, &[&str])] = &[
    ("konsole", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("kitty", &[]),
    ("alacritty", &["-e"]),
    ("wezterm", &["start", "--"]),
    ("foot", &[]),
    ("lxterminal", &["-e"]),
    ("xterm", &["-e"]),
];

/// Jeśli nie ma terminala: uruchom się w nowym oknie i zwróć jego kod wyjścia.
/// `None` = terminal jest (albo nie da się otworzyć) — kontynuuj normalnie.
pub fn relaunch_in_terminal() -> Option<i32> {
    if has_tty() || env::var_os(MARK).is_some() {
        return None;
    }
    // W AppImage current_exe() to ścieżka w tymczasowym montowaniu; ponowne uruchomienie
    // w terminalu ma użyć samego pliku AppImage ($APPIMAGE).
    let exe = match env::var_os("APPIMAGE") {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => env::current_exe().ok()?,
    };
    let args: Vec<String> = env::args().skip(1).collect();

    // Własne wskazanie terminala ma pierwszeństwo: VIDEOPIPELINE_TERMINAL="foot" itp.
    let mut candidates: Vec<(String, Vec<String>)> = vec![];
    if let Ok(t) = env::var("VIDEOPIPELINE_TERMINAL") {
        let mut it = t.split_whitespace().map(String::from);
        if let Some(prog) = it.next() {
            candidates.push((prog, it.collect()));
        }
    }
    for (prog, pre) in TERMINALS {
        candidates.push((prog.to_string(), pre.iter().map(|s| s.to_string()).collect()));
    }

    for (prog, pre) in candidates {
        if !in_path(&prog) {
            continue;
        }
        let status = Command::new(&prog).args(&pre).arg(&exe).args(&args).env(MARK, "1").status();
        match status {
            // gnome-terminal i część innych zwraca od razu po otwarciu okna — to nie błąd.
            Ok(s) => return Some(s.code().unwrap_or(0)),
            Err(_) => continue,
        }
    }
    None
}

/// Okno terminala otwarte przez nas znika po wyjściu programu — przy błędzie zostaw czas na przeczytanie.
pub fn wait_for_enter_if_relaunched() {
    if env::var_os(MARK).is_some() {
        eprintln!("\nNaciśnij Enter, aby zamknąć okno...");
        let _ = io::stderr().flush();
        let mut s = String::new();
        let _ = io::stdin().lock().read_line(&mut s);
    }
}

/// Zachowuje się jak `wait_for_enter_if_relaunched`, ale po normalnym zakończeniu nie czeka.
pub struct PauseOnExit;

impl PauseOnExit {
    pub fn new() -> Self {
        PauseOnExit
    }
}

impl Drop for PauseOnExit {
    fn drop(&mut self) {
        if std::thread::panicking() {
            wait_for_enter_if_relaunched();
        }
    }
}
