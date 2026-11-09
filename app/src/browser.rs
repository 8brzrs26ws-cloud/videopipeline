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

/// Windows: istniejące dyski poza bieżącym, jako katalogi "C:".
#[cfg(windows)]
fn other_drives(cwd: &std::path::Path) -> Vec<Entry> {
    let cur = cwd.to_string_lossy().to_uppercase();
    (b'A'..=b'Z')
        .map(|l| format!("{}:\\", l as char))
        .filter(|d| !cur.starts_with(d.as_str()) && std::path::Path::new(d).is_dir())
        .map(|d| Entry {
            name: d.trim_end_matches('\\').to_string(),
            path: PathBuf::from(&d),
            is_dir: true,
            is_up: false,
            size: 0,
            mtime: None,
        })
        .collect()
}

#[cfg(not(windows))]
fn other_drives(_cwd: &std::path::Path) -> Vec<Entry> {
    vec![]
}

pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_up: bool,
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

pub struct Browser {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub state: ListState,
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

        let mut entries = vec![];
        if let Some(parent) = self.cwd.parent() {
            entries.push(Entry {
                name: "..".into(),
                path: parent.to_path_buf(),
                is_dir: true,
                is_up: true,
                size: 0,
                mtime: None,
            });
        } else {
            // Windows: w katalogu głównym dysku pokaż pozostałe dyski (C:\ → D:\ …).
            entries.extend(other_drives(&self.cwd));
        }
        entries.extend(dirs);
        entries.extend(files);
        self.entries = entries;

        let len = self.entries.len();
        let by_name = select.and_then(|n| self.entries.iter().position(|e| e.name == n));
        let idx = by_name.or(prev.map(|i| i.min(len.saturating_sub(1)))).unwrap_or(0);
        self.state.select(if len > 0 { Some(idx) } else { None });
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.state.selected().and_then(|i| self.entries.get(i))
    }

    /// Cel operacji na plikach: zaznaczone pozycje albo bieżąca (bez "..").
    pub fn targets(&self) -> Vec<PathBuf> {
        if !self.marked.is_empty() {
            return self.marked.iter().cloned().collect();
        }
        self.selected().filter(|e| !e.is_up).map(|e| vec![e.path.clone()]).unwrap_or_default()
    }

    pub fn move_by(&mut self, d: isize) {
        let len = self.entries.len();
        if len == 0 {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let n = (cur + d).clamp(0, len as isize - 1);
        self.state.select(Some(n as usize));
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = self.cwd.parent().map(|p| p.to_path_buf()) {
            let prev = self.cwd.file_name().map(|n| n.to_string_lossy().to_string());
            self.cwd = parent;
            self.marked.clear();
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
                if !e.is_up {
                    self.marked.insert(e.path.clone());
                }
            }
        }
        self.state.select(Some(n));
    }

    /// '+' — zaznacz wszystkie pliki (bez katalogów).
    pub fn mark_all(&mut self) {
        for e in self.entries.iter().filter(|e| !e.is_up && !e.is_dir) {
            self.marked.insert(e.path.clone());
        }
    }

    /// '*' — odwróć zaznaczenie plików.
    pub fn invert_marks(&mut self) {
        for e in self.entries.iter().filter(|e| !e.is_up && !e.is_dir) {
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
            let e = self.selected()?;
            (e.is_up, e.is_dir, e.path.clone())
        };
        if is_up {
            self.go_up();
            None
        } else if is_dir {
            self.cwd = path;
            self.marked.clear();
            self.state.select(None);
            self.reload(None);
            self.state.select(if self.entries.is_empty() { None } else { Some(0) });
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
            Some(e) if !e.is_up => e.path.clone(),
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
