//! Uruchamianie z menedżera plików (np. dwuklik w Dolphinie/KDE): gdy proces nie ma terminala,
//! otwieramy własne okno terminala i uruchamiamy w nim ten sam program z tymi samymi argumentami.
//! Windows: program konsolowy uruchomiony z Eksploratora sam dostaje okno konsoli — nic nie
//! uruchamiamy ponownie, ale przy błędzie czekamy na Enter, zanim okno zniknie.
use std::{
    env,
    io::{self, BufRead, Write},
};
#[cfg(not(windows))]
use std::{io::IsTerminal, process::Command};

const MARK: &str = "VIDEOPIPELINE_IN_TERMINAL";

#[cfg(not(windows))]
fn has_tty() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

#[cfg(not(windows))]
fn in_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|p| env::split_paths(&p).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

/// (program, argumenty poprzedzające polecenie). Kolejność: KDE najpierw.
/// Okno dostaje tytuł „VideoPipeline”, a gdzie terminal na to pozwala — klasę / app-id
/// „videopipeline”, więc pasek zadań dopasuje je do videopipeline.desktop (ikona programu).
/// Konsole nie pozwala zmienić app-id (pasek zadań KDE pokaże ikonę Konsole) — tylko tytuł karty.
#[cfg(not(windows))]
const TERMINALS: &[(&str, &[&str])] = &[
    ("konsole", &["-p", "LocalTabTitleFormat=VideoPipeline", "-p", "RemoteTabTitleFormat=VideoPipeline", "-e"]),
    ("gnome-terminal", &["--title=VideoPipeline", "--"]),
    ("xfce4-terminal", &["--title=VideoPipeline", "-x"]),
    ("kitty", &["--class", "videopipeline", "--title", "VideoPipeline"]),
    ("alacritty", &["--class", "videopipeline,videopipeline", "--title", "VideoPipeline", "-e"]),
    ("wezterm", &["start", "--class", "videopipeline", "--"]),
    ("foot", &["--app-id=videopipeline", "--title=VideoPipeline"]),
    ("lxterminal", &["--title=VideoPipeline", "-e"]),
    ("xterm", &["-class", "videopipeline", "-T", "VideoPipeline", "-e"]),
];

/// Windows: konsola ma pojedynczy proces (nasz) = uruchomienie dwuklikiem z Eksploratora.
#[cfg(windows)]
fn own_console_window() -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
    }
    let mut pids = [0u32; 4];
    // SAFETY: bufor na 4 identyfikatory, rozmiar przekazany w drugim argumencie.
    let n = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    n == 1
}

#[cfg(windows)]
pub fn relaunch_in_terminal() -> Option<i32> {
    if own_console_window() {
        env::set_var(MARK, "1"); // okno zniknie razem z programem — przy błędzie poczekaj na Enter
    }
    None
}

/// Jeśli nie ma terminala: uruchom się w nowym oknie i zwróć jego kod wyjścia.
/// `None` = terminal jest (albo nie da się otworzyć) — kontynuuj normalnie.
#[cfg(not(windows))]
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
            let mut pre: Vec<String> = it.collect();
            // Sama nazwa znanego terminala: jego argumenty (tytuł, klasa okna) z listy poniżej.
            if pre.is_empty() {
                if let Some((_, p)) = TERMINALS.iter().find(|(n, _)| *n == prog) {
                    pre = p.iter().map(|s| s.to_string()).collect();
                }
            }
            candidates.push((prog, pre));
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
