//! Wygląd okna, w którym działa TUI: tytuł „VideoPipeline” (przywracany przy wyjściu),
//! na Windows ikona okna konsoli (klasyczna konsola conhost).
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
