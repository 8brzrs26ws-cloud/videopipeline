//! Wygląd okna, w którym działa TUI: tytuł „VideoPipeline” (przywracany przy wyjściu),
//! na Windows ikona okna konsoli, a w KDE reguła okna KWin, dzięki której okno Konsole
//! z VideoPipeline ma na pasku zadań ikonę programu zamiast ikony terminala.
#[cfg(not(windows))]
use std::io::Write;

pub const TITLE: &str = "VideoPipeline";

/// Ustawia tytuł (i na Windows ikonę) okna; przywraca poprzednie przy `drop`.
pub struct Look {
    #[cfg(windows)]
    win: win::Saved,
}

impl Look {
    #[cfg(not(windows))]
    pub fn apply() -> Look {
        // Zapamiętaj tytuł na stosie terminala (xterm: CSI 22;0 t), potem ustaw nasz.
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b[22;0t\x1b]0;{TITLE}\x07");
        let _ = out.flush();
        Look {}
    }

    #[cfg(windows)]
    pub fn apply() -> Look {
        Look { win: win::apply(TITLE) }
    }
}

impl Drop for Look {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        {
            // Przywróć tytuł ze stosu (terminale bez tej funkcji zignorują sekwencję).
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b[23;0t");
            let _ = out.flush();
        }
        #[cfg(windows)]
        win::restore(&self.win);
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    type Hwnd = *mut c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleWindow() -> Hwnd;
        fn GetConsoleTitleW(buf: *mut u16, size: u32) -> u32;
        fn SetConsoleTitleW(title: *const u16) -> i32;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    #[link(name = "user32")]
    extern "system" {
        fn LoadImageW(inst: *mut c_void, name: *const u16, kind: u32, cx: i32, cy: i32, flags: u32) -> *mut c_void;
        fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetSystemMetrics(index: i32) -> i32;
    }

    const WM_SETICON: u32 = 0x0080;
    const ICON_SMALL: usize = 0;
    const ICON_BIG: usize = 1;
    const IMAGE_ICON: u32 = 1;
    const SM_CXICON: i32 = 11;
    const SM_CYICON: i32 = 12;
    const SM_CXSMICON: i32 = 49;
    const SM_CYSMICON: i32 = 50;
    /// Identyfikator ikony w zasobach videopipeline.exe (app/build.rs: `1 ICON`).
    const ICON_ID: usize = 1;

    pub struct Saved {
        title: Option<Vec<u16>>,
        icons: Option<(Hwnd, isize, isize)>,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn apply(title: &str) -> Saved {
        // SAFETY: wywołania Win32 z poprawnymi buforami; uchwyty sprawdzane przed użyciem.
        unsafe {
            let mut buf = vec![0u16; 1024];
            let n = GetConsoleTitleW(buf.as_mut_ptr(), buf.len() as u32) as usize;
            let old_title = if n > 0 {
                buf.truncate(n.min(buf.len() - 1));
                buf.push(0);
                Some(buf)
            } else {
                None
            };
            SetConsoleTitleW(wide(title).as_ptr());

            // Klasyczna konsola (conhost): ikona okna = ikona programu. W Windows Terminal okno
            // należy do terminala (GetConsoleWindow zwraca niewidoczne okno zastępcze) — bez efektu.
            let hwnd = GetConsoleWindow();
            let mut icons = None;
            if !hwnd.is_null() {
                let inst = GetModuleHandleW(std::ptr::null());
                let load = |cx, cy| LoadImageW(inst, ICON_ID as *const u16, IMAGE_ICON, cx, cy, 0);
                let big = load(GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON));
                let small = load(GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
                if !big.is_null() && !small.is_null() {
                    let old_big = SendMessageW(hwnd, WM_SETICON, ICON_BIG, big as isize);
                    let old_small = SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small as isize);
                    icons = Some((hwnd, old_big, old_small));
                }
            }
            Saved { title: old_title, icons }
        }
    }

    pub fn restore(s: &Saved) {
        // SAFETY: jak wyżej; przywracamy wartości zwrócone wcześniej przez system.
        unsafe {
            if let Some(t) = &s.title {
                SetConsoleTitleW(t.as_ptr());
            }
            if let Some((hwnd, big, small)) = s.icons {
                SendMessageW(hwnd, WM_SETICON, ICON_BIG, big);
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL, small);
            }
        }
    }
}

// ------------------------------------------------------------------ KDE / KWin ---

/// Reguła okna KWin: okno Konsole z tytułem „VideoPipeline” dostaje plik videopipeline.desktop,
/// więc pasek zadań Plasmy pokazuje ikonę programu. KWin sprawdza reguły z tytułem przy otwarciu
/// okna — działa więc dla okien otwieranych przez VideoPipeline (menu, dwuklik), które startują
/// z tytułem karty „VideoPipeline” (term.rs), a nie dla już otwartego terminala.
#[cfg(unix)]
pub mod kde {
    use std::{
        path::PathBuf,
        process::{Command, Stdio},
    };

    const GROUP: &str = "videopipeline-taskbar-icon";
    /// Tytuł okna Konsole to „<tytuł karty> — Konsole”.
    const TITLE_RE: &str = "^VideoPipeline( [—–-] Konsole)?$";
    const KEYS: &[(&str, &str)] = &[
        ("Description", "VideoPipeline: ikona na pasku zadań (Konsole)"),
        ("desktopfile", "videopipeline"),
        ("desktopfilerule", "2"),
        ("title", TITLE_RE),
        ("titlematch", "3"),
        ("wmclass", "konsole"),
        ("wmclassmatch", "2"),
        ("wmclasscomplete", "false"),
        ("types", "1"),
    ];

    fn tool(names: &[&str]) -> Option<String> {
        let path = std::env::var_os("PATH")?;
        names
            .iter()
            .find(|n| std::env::split_paths(&path).any(|d| d.join(n).is_file()))
            .map(|n| n.to_string())
    }

    fn cmd(prog: &str) -> Command {
        let mut c = Command::new(prog);
        c.stdin(Stdio::null()).stderr(Stdio::null());
        crate::player::clean_env(&mut c); // bez bibliotek zapasowych AppImage
        c
    }

    struct Kcfg {
        read: String,
        write: String,
    }

    impl Kcfg {
        fn find() -> Option<Kcfg> {
            Some(Kcfg {
                read: tool(&["kreadconfig6", "kreadconfig5"])?,
                write: tool(&["kwriteconfig6", "kwriteconfig5"])?,
            })
        }
        fn get(&self, group: &str, key: &str) -> String {
            cmd(&self.read)
                .args(["--file", "kwinrulesrc", "--group", group, "--key", key])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default()
        }
        fn set(&self, group: &str, key: &str, value: &str) -> bool {
            cmd(&self.write)
                .args(["--file", "kwinrulesrc", "--group", group, "--key", key, value])
                .stdout(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        }
        fn delete(&self, group: &str, key: &str) {
            let _ = cmd(&self.write)
                .args(["--file", "kwinrulesrc", "--group", group, "--key", key, "--delete"])
                .stdout(Stdio::null())
                .status();
        }
        fn rules(&self) -> Vec<String> {
            self.get("General", "rules").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
        }
        fn set_rules(&self, list: &[String]) {
            self.set("General", "rules", &list.join(","));
            self.set("General", "count", &list.len().to_string());
        }
    }

    fn reconfigure() {
        if let Some(t) = tool(&["dbus-send"]) {
            let _ = cmd(&t)
                .args(["--session", "--type=method_call", "--dest=org.kde.KWin", "/KWin", "org.kde.KWin.reconfigure"])
                .stdout(Stdio::null())
                .status();
        } else if let Some(t) = tool(&["qdbus6", "qdbus"]) {
            let _ = cmd(&t).args(["org.kde.KWin", "/KWin", "reconfigure"]).stdout(Stdio::null()).status();
        }
    }

    pub fn is_kde() -> bool {
        std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|x| x.eq_ignore_ascii_case("KDE")))
    }

    /// Plik videopipeline.desktop zainstalowany w systemie (deb, pakiet Arch, install.sh) —
    /// bez niego Plasma nie ma skąd wziąć ikony. Pliki z wnętrza AppImage się nie liczą.
    pub fn desktop_file_installed() -> bool {
        let home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| videopipeline_core::platform::home_dir().map(|h| h.join(".local/share")));
        let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        let appdir = std::env::var_os("APPDIR").map(PathBuf::from);
        home.into_iter()
            .chain(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from))
            .filter(|d| appdir.as_ref().is_none_or(|a| !d.starts_with(a)))
            .any(|d| d.join("applications/videopipeline.desktop").is_file())
    }

    fn opt_out_marker() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| videopipeline_core::platform::home_dir().map(|h| h.join(".config")))?;
        Some(base.join("videopipeline/no-kde-taskbar-rule"))
    }

    /// Dodaje (albo aktualizuje) regułę. `Ok(true)` = zmieniono konfigurację KWin.
    pub fn install() -> Result<bool, String> {
        let k = Kcfg::find().ok_or("brak kreadconfig6/kwriteconfig6 (to nie jest sesja KDE Plasma?)")?;
        let mut rules = k.rules();
        let present = rules.iter().any(|r| r == GROUP);
        if present && KEYS.iter().all(|(key, v)| k.get(GROUP, key) == *v) {
            return Ok(false);
        }
        for (key, v) in KEYS {
            if !k.set(GROUP, key, v) {
                return Err(format!("nie udało się zapisać ~/.config/kwinrulesrc ({})", k.write));
            }
        }
        if !present {
            rules.push(GROUP.to_string());
            k.set_rules(&rules);
        }
        reconfigure();
        Ok(true)
    }

    /// Usuwa regułę. `Ok(true)` = była i została usunięta.
    pub fn remove() -> Result<bool, String> {
        let k = Kcfg::find().ok_or("brak kreadconfig6/kwriteconfig6")?;
        let mut rules = k.rules();
        let present = rules.iter().any(|r| r == GROUP);
        for (key, _) in KEYS {
            k.delete(GROUP, key);
        }
        if present {
            rules.retain(|r| r != GROUP);
            k.set_rules(&rules);
            reconfigure();
        }
        Ok(present)
    }

    /// Przy starcie TUI: w KDE dodaj regułę po cichu, chyba że użytkownik ją wyłączył (--kde-icon off).
    pub fn auto_install() {
        if !is_kde() || !desktop_file_installed() || opt_out_marker().is_some_and(|m| m.exists()) {
            return;
        }
        let _ = install();
    }

    /// `--kde-icon on|off`.
    pub fn set_enabled(on: bool) -> Result<String, String> {
        let marker = opt_out_marker();
        if on {
            if let Some(m) = &marker {
                let _ = std::fs::remove_file(m);
            }
            let changed = install()?;
            let mut msg = if changed {
                "Dodano regułę okna KWin: Konsole z VideoPipeline pokaże na pasku zadań ikonę programu.".to_string()
            } else {
                "Reguła okna KWin jest już zainstalowana.".to_string()
            };
            if !desktop_file_installed() {
                msg.push_str("\nUWAGA: nie znaleziono videopipeline.desktop w menu aplikacji — bez niego Plasma nie znajdzie ikony.");
            }
            Ok(msg)
        } else {
            if let Some(m) = &marker {
                if let Some(d) = m.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                let _ = std::fs::write(m, "VideoPipeline nie dodaje reguły okna KWin (videopipeline --kde-icon on, aby przywrócić)\n");
            }
            Ok(if remove()? {
                "Usunięto regułę okna KWin VideoPipeline.".into()
            } else {
                "Reguły okna KWin VideoPipeline nie było.".into()
            })
        }
    }
}
