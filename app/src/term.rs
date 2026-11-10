//! Uruchamianie z menu lub menedżera plików (np. dwuklik w Dolphinie/Eksploratorze): gdy proces nie ma
//! terminala, otwieramy własne okno i uruchamiamy w nim ten sam program z tymi samymi argumentami.
//! Najpierw terminal dołączony do paczek (Alacritty jako videopipeline-term, z ikoną VideoPipeline
//! na pasku zadań i w ramce okna), potem terminale z systemu.
//! Windows: bez dołączonego terminala program zostaje w oknie konsoli, które dał mu system,
//! a przy błędzie czeka na Enter, zanim okno zniknie.
use std::{
    env,
    ffi::OsString,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
#[cfg(not(windows))]
use std::io::IsTerminal;

const MARK: &str = "VIDEOPIPELINE_IN_TERMINAL";

/// Terminal dołączony do paczek (obok binarki): program i argumenty poprzedzające polecenie.
fn bundled_terminal() -> Option<(PathBuf, Vec<OsString>)> {
    let dir = env::current_exe().ok()?.parent()?.to_path_buf();
    let term = dir.join(format!("videopipeline-term{}", env::consts::EXE_SUFFIX));
    if !term.is_file() {
        return None;
    }
    let mut pre: Vec<OsString> = vec![];
    let cfg = dir.join("videopipeline-term.toml");
    if cfg.is_file() {
        pre.push("--config-file".into());
        pre.push(cfg.into_os_string());
    }
    for a in ["--class", "videopipeline,videopipeline", "--title", "VideoPipeline", "-e"] {
        pre.push(a.into());
    }
    Some((term, pre))
}

/// Uruchamia dołączony terminal z programem. `None`, gdy okno nie wystartowało (np. brak OpenGL
/// w maszynie wirtualnej) — wtedy próbujemy dalej. `wait`: czekaj na zamknięcie okna.
fn spawn_bundled(term: &Path, pre: &[OsString], exe: &Path, args: &[OsString], wait: bool) -> Option<i32> {
    let mut child = Command::new(term).args(pre).arg(exe).args(args).env(MARK, "1").spawn().ok()?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        match child.try_wait() {
            Ok(Some(st)) => return st.success().then_some(0),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    if wait {
        Some(child.wait().ok()?.code().unwrap_or(0))
    } else {
        Some(0)
    }
}

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

/// Windows: konsola widoczna/ukryta (okno conhost; w Windows Terminal bez efektu).
#[cfg(windows)]
fn show_console(show: bool) {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
    }
    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: *mut std::ffi::c_void, cmd: i32) -> i32;
    }
    // SAFETY: uchwyt okna konsoli sprawdzony przed użyciem.
    unsafe {
        let h = GetConsoleWindow();
        if !h.is_null() {
            ShowWindow(h, if show { 5 } else { 0 }); // SW_SHOW / SW_HIDE
        }
    }
}

#[cfg(windows)]
pub fn relaunch_in_terminal() -> Option<i32> {
    // Już w naszym oknie albo uruchomiony z istniejącej konsoli (cmd, PowerShell) — zostajemy.
    if env::var_os(MARK).is_some() || !own_console_window() {
        return None;
    }
    if let Some((term, pre)) = bundled_terminal() {
        let exe = env::current_exe().ok()?;
        let args: Vec<OsString> = env::args_os().skip(1).collect();
        show_console(false);
        // Nie czekamy na zamknięcie okna: konsola, którą dał system, ma zniknąć od razu.
        if let Some(code) = spawn_bundled(&term, &pre, &exe, &args, false) {
            return Some(code);
        }
        show_console(true);
    }
    env::set_var(MARK, "1"); // okno zniknie razem z programem — przy błędzie poczekaj na Enter
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
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => env::current_exe().ok()?,
    };
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    // Własne wskazanie terminala ma pierwszeństwo: VIDEOPIPELINE_TERMINAL="foot" itp.
    let custom = env::var("VIDEOPIPELINE_TERMINAL").ok().and_then(|t| {
        let mut it = t.split_whitespace().map(String::from);
        let prog = it.next()?;
        let mut pre: Vec<String> = it.collect();
        // Sama nazwa znanego terminala: jego argumenty (tytuł, klasa okna) z listy poniżej.
        if pre.is_empty() {
            if let Some((_, p)) = TERMINALS.iter().find(|(n, _)| *n == prog) {
                pre = p.iter().map(|s| s.to_string()).collect();
            }
        }
        Some((prog, pre))
    });
    let run = |prog: &str, pre: &[String]| -> Option<i32> {
        if !in_path(prog) {
            return None;
        }
        // gnome-terminal i część innych zwraca od razu po otwarciu okna — to nie błąd.
        let st = Command::new(prog).args(pre).arg(&exe).args(&args).env(MARK, "1").status().ok()?;
        Some(st.code().unwrap_or(0))
    };

    if let Some((prog, pre)) = &custom {
        if let Some(code) = run(prog, pre) {
            return Some(code);
        }
    }
    // Terminal z paczki (AppImage, deb, Arch) — ikona VideoPipeline na pasku zadań.
    if let Some((term, pre)) = bundled_terminal() {
        if let Some(code) = spawn_bundled(&term, &pre, &exe, &args, true) {
            return Some(code);
        }
    }
    for (prog, pre) in TERMINALS {
        let pre: Vec<String> = pre.iter().map(|s| s.to_string()).collect();
        if let Some(code) = run(prog, &pre) {
            return Some(code);
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
