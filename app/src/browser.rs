//! Lewy panel (przeglądarka plików) i prawy panel (kolejka).
use ratatui::widgets::ListState;
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    time::SystemTime,
};

pub use videopipeline_core::files::{is_video, videos_in};

/// Zakres zaznaczany przez Shift+ruch: jak w MC — zaznacza bieżący i przesuwa kursor.
/// Zwraca (zakres indeksów do zaznaczenia, nowa pozycja kursora).
fn shift_range(cur: usize, d: isize, len: usize) -> (std::ops::Range<usize>, usize) {
    let n = (cur as isize + d).clamp(0, len as isize - 1) as usize;
    let range = if d > 0 {
        cur..(if n == len - 1 { len } else { n })
    } else {
        (if n == 0 { 0 } else { n + 1 })..cur + 1
    };
    (range, n)
}

/// Dysk / zamontowany system plików pokazywany pod zawartością katalogu.
#[derive(Clone, Default)]
pub struct DriveInfo {
    pub label: Option<String>,
    pub total: Option<u64>,
    pub free: Option<u64>,
}

/// Windows: dyski C:, D: … z etykietą woluminu i miejscem.
#[cfg(windows)]
fn drives() -> Vec<(PathBuf, String, DriveInfo)> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_comp: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_len: u32,
        ) -> i32;
        fn GetDiskFreeSpaceExW(root: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        fn GetLogicalDrives() -> u32;
    }
    // SAFETY: maska dysków z systemu; bufory o podanych rozmiarach, ścieżki zakończone zerem.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = vec![];
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        if !std::path::Path::new(&root).is_dir() {
            continue; // np. czytnik kart / napęd bez nośnika
        }
        let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        let mut name = [0u16; 261];
        let label = unsafe {
            GetVolumeInformationW(
                w.as_ptr(),
                name.as_mut_ptr(),
                name.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        } != 0;
        let label = label
            .then(|| String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(0)]))
            .filter(|l| !l.is_empty());
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) } != 0;
        out.push((
            PathBuf::from(&root),
            format!("{letter}:"),
            DriveInfo { label, total: ok.then_some(total), free: ok.then_some(avail) },
        ));
    }
    out
}

/// Linux: zamontowane systemy plików z /proc/self/mountinfo — dyski lokalne, pendrive'y
/// (/run/media), udziały sieciowe; bez systemowych (/proc, /sys, /boot, /snap…).
/// Podwolumeny btrfs jednego dysku (/, /home, /var/cache…) jako jedna pozycja (najkrótsza ścieżka).
#[cfg(target_os = "linux")]
fn drives() -> Vec<(PathBuf, String, DriveInfo)> {
    use std::collections::BTreeMap;
    const FS: &[&str] = &[
        "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "bcachefs", "zfs", "vfat", "exfat", "ntfs", "ntfs3",
        "fuseblk", "nfs", "nfs4", "cifs", "smb3", "9p", "virtiofs", "fuse.sshfs", "fuse.rclone",
    ];
    const HIDDEN: &[&str] = &[
        "/proc", "/sys", "/dev", "/run", "/snap", "/boot", "/efi", "/tmp", "/usr", "/var", "/mnt/wslg", "/init",
    ];
    // \040 → spacja itd. (mountinfo zapisuje znaki specjalne ósemkowo).
    fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
                out.push(u8::from_str_radix(&s[i + 1..i + 4], 8).unwrap_or(b'?'));
                i += 4;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    let hidden = |mp: &str| {
        !mp.starts_with("/run/media/")
            && HIDDEN.iter().any(|h| mp == *h || mp.starts_with(&format!("{h}/")))
    };
    // Etykiety: /dev/disk/by-label/<etykieta> → urządzenie.
    let mut labels: BTreeMap<PathBuf, String> = BTreeMap::new();
    if let Ok(rd) = fs::read_dir("/dev/disk/by-label") {
        for e in rd.flatten() {
            if let Ok(dev) = fs::canonicalize(e.path()) {
                let name = e.file_name().to_string_lossy().replace("\\x20", " ");
                labels.insert(dev, name);
            }
        }
    }
    let Ok(text) = fs::read_to_string("/proc/self/mountinfo") else { return vec![] };
    // klucz: urządzenie (major:minor) albo źródło udziału sieciowego → (punkt montowania, źródło, lokalny?)
    let mut best: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let Some(dash) = f.iter().position(|x| *x == "-") else { continue };
        if dash < 5 || f.len() < dash + 3 {
            continue;
        }
        let (fstype, source, mp) = (f[dash + 1], unescape(f[dash + 2]), unescape(f[4]));
        if !FS.contains(&fstype) || hidden(&mp) {
            continue;
        }
        let local = source.starts_with("/dev/");
        let key = if local { f[2].to_string() } else { source.clone() };
        let better = |old: &str| {
            let rank = |p: &str| (p != "/", p.matches('/').count(), p.len());
            rank(&mp) < rank(old)
        };
        if best.get(&key).is_none_or(|(old, _, _)| better(old)) {
            best.insert(key, (mp, source, local));
        }
    }
    let mut out: Vec<(PathBuf, String, DriveInfo)> = best
        .into_values()
        .map(|(mp, source, local)| {
            let label = fs::canonicalize(&source).ok().and_then(|d| labels.get(&d).cloned());
            // Rozmiar tylko dla dysków lokalnych — statvfs na zerwanym udziale sieciowym potrafi zawisnąć.
            let (total, free) = if local { statvfs(&mp) } else { (None, None) };
            let label = label.or_else(|| (!local).then(|| source.clone()));
            (PathBuf::from(&mp), mp, DriveInfo { label, total, free })
        })
        .collect();
    out.sort_by(|a, b| (a.1 != "/", &a.1).cmp(&(b.1 != "/", &b.1)));
    out
}

#[cfg(target_os = "linux")]
fn statvfs(path: &str) -> (Option<u64>, Option<u64>) {
    let Ok(c) = std::ffi::CString::new(path) else { return (None, None) };
    // SAFETY: ścieżka zakończona zerem, struktura wypełniana przez libc.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return (None, None);
        }
        let bs = st.f_frsize as u64;
        (Some(st.f_blocks as u64 * bs), Some(st.f_bavail as u64 * bs))
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn drives() -> Vec<(PathBuf, String, DriveInfo)> {
    vec![]
}

pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_up: bool,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// Pozycja z listy dysków (pod zawartością katalogu).
    pub drive: Option<DriveInfo>,
    /// Linia oddzielająca zawartość katalogu od listy dysków (nie da się jej wybrać).
    pub sep: bool,
}

impl Entry {
    /// "..", separator i dyski — nie są plikami/katalogami do zaznaczania, usuwania czy kolejki.
    pub fn is_special(&self) -> bool {
        self.is_up || self.sep || self.drive.is_some()
    }
}

pub struct Browser {
    pub cwd: PathBuf,
    /// Najpierw dyski i separator (`pinned` pozycji, przypięte u góry panelu), potem zawartość katalogu.
    pub entries: Vec<Entry>,
    /// Bieżąca pozycja (indeks w `entries`).
    pub state: ListState,
    /// Liczba pozycji przypiętych u góry (dyski + separator); 0 = brak.
    pub pinned: usize,
    /// Przewijana część listy (zawartość katalogu) — indeksy od `pinned`.
    pub rest: ListState,
    /// Ile przypiętych wierszy zmieściło się w ostatnim rysowaniu (obsługa myszy).
    pub pinned_shown: usize,
    pub marked: BTreeSet<PathBuf>,
    pub error: Option<String>,
    pub page: usize,
}

impl Browser {
    pub fn new(start: PathBuf) -> Self {
        let mut b = Browser {
            cwd: start,
            entries: vec![],
            state: ListState::default(),
            pinned: 0,
            rest: ListState::default(),
            pinned_shown: 0,
            marked: BTreeSet::new(),
            error: None,
            page: 10,
        };
        b.reload(None);
        b
    }

    pub fn reload(&mut self, select: Option<&str>) {
        self.error = None;
        let prev = self.state.selected();
        let mut dirs: Vec<Entry> = vec![];
        let mut files: Vec<Entry> = vec![];

        match fs::read_dir(&self.cwd) {
            Ok(rd) => {
                for e in rd.filter_map(|e| e.ok()) {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    let path = e.path();
                    let md = fs::metadata(&path).ok(); // podąża za symlinkami
                    let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let entry = Entry {
                        name,
                        size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                        mtime: md.as_ref().and_then(|m| m.modified().ok()),
                        path: path.clone(),
                        is_dir,
                        is_up: false,
                        drive: None,
                        sep: false,
                    };
                    if is_dir {
                        dirs.push(entry);
                    } else if is_video(&path) {
                        files.push(entry);
                    }
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        dirs.sort_by_key(|e| e.name.to_lowercase());
        files.sort_by_key(|e| e.name.to_lowercase());

        // Na górze, przypięte i oddzielone linią: dyski (Windows) / zamontowane systemy plików (Linux).
        let mut entries = vec![];
        for (path, name, info) in drives() {
            entries.push(Entry {
                name,
                path,
                is_dir: true,
                is_up: false,
                size: info.total.unwrap_or(0),
                mtime: None,
                drive: Some(info),
                sep: false,
            });
        }
        if !entries.is_empty() {
            entries.push(Entry {
                name: String::new(),
                path: PathBuf::new(),
                is_dir: false,
                is_up: false,
                size: 0,
                mtime: None,
                drive: None,
                sep: true,
            });
        }
        self.pinned = entries.len();
        if let Some(parent) = self.cwd.parent() {
            entries.push(Entry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                is_up: true,
                size: 0,
                mtime: None,
                drive: None,
                sep: false,
            });
        }
        entries.extend(dirs);
        entries.extend(files);
        self.entries = entries;

        let len = self.entries.len();
        let by_name = select.and_then(|n| self.entries.iter().position(|e| e.name == n && e.drive.is_none()));
        // Domyślnie pierwsza pozycja katalogu ("..") — nie dysk.
        let first = self.pinned.min(len.saturating_sub(1));
        let mut idx = by_name.or(prev.map(|i| i.min(len.saturating_sub(1)))).unwrap_or(first);
        if self.entries.get(idx).is_some_and(|e| e.sep) {
            idx = first;
        }
        self.state.select(if len > 0 { Some(idx) } else { None });
    }

    /// Wiersz listy (od góry, z przypiętymi dyskami) → indeks w `entries`; None poza listą.
    pub fn index_at(&self, row: usize) -> Option<usize> {
        let i = if row < self.pinned_shown {
            row
        } else {
            self.pinned + self.rest.offset() + (row - self.pinned_shown)
        };
        (i < self.entries.len()).then_some(i)
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.state.selected().and_then(|i| self.entries.get(i))
    }

    /// Cel operacji na plikach: zaznaczone pozycje albo bieżąca (bez "..").
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.marked.is_empty() {
            return self.marked.iter().cloned().collect();
        }
        self.selected().filter(|e| !e.is_special()).map(|e| vec![e.path.clone()]).unwrap_or_default()
    }

    pub fn move_by(&mut self, d: isize) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let mut n = (cur + d).clamp(0, len as isize - 1);
        // Separatora nie da się wybrać: przeskocz go w kierunku ruchu (na krańcu listy — cofnij).
        if self.entries[n as usize].sep {
            let step = if d >= 0 { 1 } else { -1 };
            n = if (0..len as isize).contains(&(n + step)) { n + step } else { n - step };
        }
        self.state.select(Some(n.clamp(0, len as isize - 1) as usize));
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) {
            let prev = self.cwd.file_name().map(|n| n.to_string_lossy().to_string());
            self.cwd = parent;
            self.marked.clear();
            self.rest = ListState::default();
            self.reload(prev.as_deref());
        }
    }

    /// Shift+ruch: zaznacz od bieżącego w kierunku ruchu i przesuń kursor.
    pub fn select_move(&mut self, d: isize) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0);
        let (range, n) = shift_range(cur, d, len);
        for i in range {
            if let Some(e) = self.entries.get(i) {
                if !e.is_special() {
                    self.marked.insert(e.path.clone());
                }
            }
        }
        self.state.select(Some(n));
    }

    /// '+' — zaznacz wszystkie pliki (bez katalogów).
    pub fn mark_all(&mut self) {
        for e in self.entries.iter().filter(|e| !e.is_special() && !e.is_dir) {
            self.marked.insert(e.path.clone());
        }
    }

    /// '*' — odwróć zaznaczenie plików.
    pub fn invert_marks(&mut self) {
        for e in self.entries.iter().filter(|e| !e.is_special() && !e.is_dir) {
            if !self.marked.remove(&e.path) {
                self.marked.insert(e.path.clone());
            }
        }
    }

    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    /// Enter: katalog -> wejdź; plik -> zwróć ścieżkę.
    pub fn activate(&mut self) -> Option<PathBuf> {
        let (is_up, is_dir, path) = {
            let e = self.selected().filter(|e| !e.sep)?;
            (e.is_up, e.is_dir, e.path.clone())
        };
        if is_up {
            self.go_up();
            None
        } else if is_dir {
            self.cwd = path;
            self.marked.clear();
            self.rest = ListState::default();
            self.state.select(None);
            self.reload(None);
            None
        } else {
            Some(path)
        }
    }

    pub fn toggle_mark(&mut self) {
        if self.toggle_mark_here() {
            self.move_by(1);
        }
    }

    /// Zaznacza/odznacza bieżącą pozycję bez przesuwania kursora (kliknięcie myszą).
    pub fn toggle_mark_here(&mut self) -> bool {
        let path = match self.selected() {
            Some(e) if !e.is_special() => e.path.clone(),
            _ => return false,
        };
        if !self.marked.remove(&path) {
            self.marked.insert(path);
        }
        true
    }
}

// ---------------------------------------------------------------------------

pub struct QItem {
    pub path: PathBuf,
    pub size: u64,
}

pub enum AddResult {
    Added,
    Duplicate,
    NameClash,
}

pub struct Queue {
    pub items: Vec<QItem>,
    pub state: ListState,
    pub marked: BTreeSet<PathBuf>,
    pub page: usize,
}

impl Queue {
    pub fn new() -> Self {
        Queue { items: vec![], state: ListState::default(), marked: BTreeSet::new(), page: 10 }
    }

    pub fn add(&mut self, path: PathBuf) -> AddResult {
        if self.items.iter().any(|q| q.path == path) {
            return AddResult::Duplicate;
        }
        // Pipeline kluczuje stan po nazwie pliku — dwie takie same nazwy by się zderzyły.
        if self.items.iter().any(|q| q.path.file_name() == path.file_name()) {
            return AddResult::NameClash;
        }
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.items.push(QItem { path, size });
        if self.state.selected().is_none() {
            self.state.select(Some(0));
        }
        AddResult::Added
    }

    fn clamp_selection(&mut self) {
        let len = self.items.len();
        if len == 0 {
            self.state.select(None);
        } else {
            let i = self.state.selected().unwrap_or(0).min(len - 1);
            self.state.select(Some(i));
        }
    }

    /// Usuwa zaznaczone pozycje albo bieżącą. Zwraca liczbę usuniętych.
    pub fn remove_marked_or_current(&mut self) -> usize {
        let before = self.items.len();
        if !self.marked.is_empty() {
            let marked = std::mem::take(&mut self.marked);
            self.items.retain(|q| !marked.contains(&q.path));
        } else if let Some(i) = self.state.selected() {
            if i < self.items.len() {
                self.items.remove(i);
            }
        }
        self.clamp_selection();
        before - self.items.len()
    }

    pub fn move_by(&mut self, d: isize) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let n = (cur + d).clamp(0, len as isize - 1);
        self.state.select(Some(n as usize));
    }

    pub fn toggle_mark(&mut self) {
        let Some(i) = self.state.selected() else { return };
        let Some(q) = self.items.get(i) else { return };
        let path = q.path.clone();
        if !self.marked.remove(&path) {
            self.marked.insert(path);
        }
        self.move_by(1);
    }

    pub fn select_move(&mut self, d: isize) {
        let len = self.items.len();
        if len == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0);
        let (range, n) = shift_range(cur, d, len);
        for i in range {
            if let Some(q) = self.items.get(i) {
                self.marked.insert(q.path.clone());
            }
        }
        self.state.select(Some(n));
    }

    pub fn mark_all(&mut self) {
        for q in &self.items {
            self.marked.insert(q.path.clone());
        }
    }

    pub fn invert_marks(&mut self) {
        for q in &self.items {
            if !self.marked.remove(&q.path) {
                self.marked.insert(q.path.clone());
            }
        }
    }

    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    pub fn total_size(&self) -> u64 {
        self.items.iter().map(|q| q.size).sum()
    }

    /// Usuwa z kolejki podane ścieżki (pliki skasowane z dysku) oraz wszystko, co leżało w usuniętych katalogach.
    pub fn remove_paths(&mut self, paths: &[PathBuf]) {
        let gone = |p: &std::path::Path| paths.iter().any(|d| p.starts_with(d));
        self.items.retain(|q| !gone(&q.path));
        self.marked.retain(|p| !gone(p));
        self.clamp_selection();
    }

    /// Plik w kolejce zmienił nazwę na dysku.
    pub fn rename_path(&mut self, old: &std::path::Path, new: &std::path::Path) {
        for q in self.items.iter_mut().filter(|q| q.path == old) {
            q.path = new.to_path_buf();
        }
        if self.marked.remove(old) {
            self.marked.insert(new.to_path_buf());
        }
    }

    pub fn retain_by_name(&mut self, keep: impl Fn(&str) -> bool) {
        self.items.retain(|q| {
            q.path.file_name().map(|n| keep(&n.to_string_lossy())).unwrap_or(true)
        });
        self.marked.clear();
        self.clamp_selection();
    }
}
