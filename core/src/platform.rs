//! Różnice między Linuksem a Windows: zatrzymywanie procesów, wyłączanie komputera,
//! katalog domowy, domyślny katalog wyjściowy, nazwy plików wykonywalnych.
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Nazwa programu z rozszerzeniem platformy ("ffmpeg" → "ffmpeg.exe" na Windows).
pub fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Katalog domowy (HOME na Linuksie, USERPROFILE na Windows).
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Domyślny katalog wyników: ~/Wideo (Linux), %USERPROFILE%\Videos (Windows).
pub fn default_output_dir() -> PathBuf {
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(if cfg!(windows) { "Videos" } else { "Wideo" })
}

/// "~/x" albo "~\x" → katalog domowy + x.
pub fn expand_home(s: &str) -> PathBuf {
    let rest = s.strip_prefix("~/").or_else(|| if cfg!(windows) { s.strip_prefix("~\\") } else { None });
    match (rest, home_dir()) {
        (Some(r), Some(h)) => h.join(r),
        _ => PathBuf::from(s),
    }
}

/// Zatrzymuje proces: łagodnie (`force = false`) albo bezwarunkowo.
/// Linux: SIGTERM / SIGKILL. Windows: taskkill (z /T — razem z procesami potomnymi).
pub fn kill_pid(pid: u32, force: bool) {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T"]);
        if force {
            c.arg("/F");
        }
        c
    } else {
        let mut c = Command::new("kill");
        c.arg(if force { "-KILL" } else { "-TERM" }).arg(pid.to_string());
        c
    };
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Wyłącza komputer.
pub fn shutdown_now() {
    let _ = if cfg!(windows) {
        Command::new("shutdown").args(["/s", "/t", "0"]).status()
    } else {
        Command::new("shutdown").arg("now").status()
    };
}

/// Opis komendy wyłączenia do pomocy/opcji.
pub fn shutdown_label() -> &'static str {
    if cfg!(windows) {
        "shutdown /s /t 0"
    } else {
        "shutdown now"
    }
}

/// Czy w katalogu leży dany program (z rozszerzeniem platformy).
pub fn has_exe(dir: &Path, name: &str) -> bool {
    dir.join(exe_name(name)).is_file()
}
