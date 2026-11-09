//! F3: odtwarzanie pliku w zewnętrznym odtwarzaczu mpv (tylko TUI, tylko ze środowiskiem graficznym).
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Uruchamia mpv z plikiem w tle (program nie czeka na zamknięcie odtwarzacza).
/// Zwraca komunikat do paska stanu.
pub fn play(file: &Path) -> Result<String, String> {
    if !has_display() {
        return Err("Brak środowiska graficznego (DISPLAY/WAYLAND_DISPLAY) — mpv nie ma gdzie wyświetlić obrazu".into());
    }
    let mpv = find_mpv().ok_or_else(|| {
        "Nie znaleziono mpv — paczki VideoPipeline go zawierają; przy buildzie ze źródeł zainstaluj mpv w systemie"
            .to_string()
    })?;
    let mut cmd = Command::new(&mpv);
    cmd.arg("--force-window=yes").arg("--").arg(file);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // mpv z paczki ma swoje biblioteki obok (rpath) i może potrzebować zapasowych kopii z AppImage;
    // mpv z systemu nie może ich widzieć.
    if !is_bundled(&mpv) {
        clean_env(&mut cmd);
    }
    #[cfg(unix)]
    {
        // Własna grupa procesów: zamknięcie terminala ani Ctrl+C w TUI nie zamyka odtwarzacza.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    let mut child = cmd.spawn().map_err(|e| format!("Nie udało się uruchomić mpv: {e}"))?;
    // Odbierz kod zakończenia w tle, żeby nie zostawał proces-zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Ok(format!("Odtwarzanie w mpv: {name}"))
}

fn has_display() -> bool {
    if cfg!(windows) {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()))
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
}

/// mpv dołączony do paczki (leży obok binarki videopipeline).
fn is_bundled(mpv: &Path) -> bool {
    exe_dir().is_some_and(|d| mpv.parent() == Some(d.as_path()))
}

/// Najpierw mpv z paczki (obok binarki), potem z PATH, a na Windows także z typowych miejsc instalacji.
fn find_mpv() -> Option<PathBuf> {
    let exe = format!("mpv{}", std::env::consts::EXE_SUFFIX);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(d) = exe_dir() {
        dirs.insert(0, d);
    }
    if cfg!(windows) {
        for (var, sub) in [
            ("ProgramFiles", "mpv"),
            ("LOCALAPPDATA", "Programs\\mpv"),
            ("USERPROFILE", "scoop\\apps\\mpv\\current"),
            ("ChocolateyInstall", "lib\\mpvio.install\\tools"),
        ] {
            if let Some(base) = std::env::var_os(var) {
                dirs.push(PathBuf::from(base).join(sub));
            }
        }
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

/// AppImage podsuwa własne kopie bibliotek (LD_LIBRARY_PATH z katalogiem videopipeline-libs) —
/// mpv z systemu musi ich nie widzieć.
pub(crate) fn clean_env(cmd: &mut Command) {
    if let Some(v) = std::env::var_os("LD_LIBRARY_PATH") {
        let kept: Vec<PathBuf> = std::env::split_paths(&v)
            .filter(|p| !p.to_string_lossy().contains("videopipeline-libs"))
            .collect();
        if kept.is_empty() {
            cmd.env_remove("LD_LIBRARY_PATH");
        } else if let Ok(j) = std::env::join_paths(kept) {
            cmd.env("LD_LIBRARY_PATH", j);
        }
    }
}
