//! Ctrl+F: wyszukiwanie plików po nazwie (fragment nazwy, bez rozróżniania wielkości liter),
//! w bieżącym katalogu albo także w podkatalogach. Szukanie działa w osobnym wątku — okno
//! odświeża się w trakcie i można je przerwać. Wyniki: zaznaczanie jak w lewym panelu,
//! do kolejki trafiają tylko pliki wideo.
use crate::app::TextInput;
use ratatui::{layout::Rect, widgets::ListState};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use videopipeline_core::files::is_video;

/// Najwięcej wyników jednego wyszukiwania (dalsze są pomijane z komunikatem).
const MAX_HITS: usize = 5000;

pub struct Hit {
    pub path: PathBuf,
    /// Katalog względem miejsca wyszukiwania ("" = to samo miejsce).
    pub rel_dir: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Dopasowany fragment nazwy (indeksy znaków) — podkreślany w wynikach.
    pub mstart: usize,
    pub mlen: usize,
    /// 0 = nazwa jedyna w swojej pisowni; 1, 2… = jedna z nazw różniących się tylko wielkością liter
    /// (każda pisownia innym kolorem).
    pub variant: usize,
}

impl Hit {
    pub fn is_video(&self) -> bool {
        !self.is_dir && is_video(&self.path)
    }
}

#[derive(Default)]
struct Shared {
    found: Vec<(PathBuf, bool, u64)>,
    scanned: usize,
    done: bool,
    truncated: bool,
}

struct Job {
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
}

/// Pola okna: nazwa, przełączniki, lista wyników.
pub const F_INPUT: usize = 0;
pub const F_RECURSIVE: usize = 1;
pub const F_SHOW: usize = 2;
pub const F_AUTO: usize = 3;
pub const F_RESULTS: usize = 4;

pub struct FindView {
    pub root: PathBuf,
    pub input: TextInput,
    pub recursive: bool,
    pub show: bool,
    pub auto_add: bool,
    pub field: usize,
    pub hits: Vec<Hit>,
    pub state: ListState,
    pub marked: BTreeSet<usize>,
    /// Wyszukiwany tekst (z ostatniego Enter) i stan szukania.
    pub query: String,
    pub scanned: usize,
    pub searching: bool,
    pub truncated: bool,
    pub started: Option<Instant>,
    /// Komunikat w oknie (np. ile dodano do kolejki).
    pub info: String,
    job: Option<Job>,
    /// Obszar listy wyników z ostatniego rysowania (mysz) i jej wysokość (PgUp/PgDn).
    pub list_area: Rect,
    pub form_area: Rect,
    pub page: usize,
    pub last_click: Option<(usize, Instant)>,
    /// Wyniki do automatycznego dodania po zakończeniu szukania.
    auto_pending: bool,
}

impl FindView {
    pub fn new(root: PathBuf) -> Self {
        FindView {
            root,
            input: TextInput { chars: vec![], cursor: 0 },
            recursive: true,
            show: true,
            auto_add: false,
            field: F_INPUT,
            hits: vec![],
            state: ListState::default(),
            marked: BTreeSet::new(),
            query: String::new(),
            scanned: 0,
            searching: false,
            truncated: false,
            started: None,
            info: String::new(),
            job: None,
            list_area: Rect::default(),
            form_area: Rect::default(),
            page: 10,
            last_click: None,
            auto_pending: false,
        }
    }

    /// Enter: nowe wyszukiwanie (poprzednie, jeśli trwa, jest przerywane).
    pub fn start(&mut self) {
        self.stop();
        let q = self.input.text().trim().to_string();
        if q.is_empty() {
            self.info = "Wpisz fragment nazwy pliku".into();
            return;
        }
        self.query = q.clone();
        self.hits.clear();
        self.marked.clear();
        self.state = ListState::default();
        self.scanned = 0;
        self.truncated = false;
        self.info.clear();
        self.searching = true;
        self.started = Some(Instant::now());
        self.auto_pending = self.auto_add;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c, root, rec) = (shared.clone(), cancel.clone(), self.root.clone(), self.recursive);
        std::thread::spawn(move || search(&root, &q.to_lowercase(), rec, &s, &c));
        self.job = Some(Job { shared, cancel });
    }

    pub fn stop(&mut self) {
        if let Some(j) = self.job.take() {
            j.cancel.store(true, Ordering::SeqCst);
        }
        self.searching = false;
    }

    /// Przejmuje nowe wyniki z wątku (wołane w każdej klatce). Zwraca pliki wideo do dodania
    /// do kolejki, gdy szukanie się skończyło, a opcja „dodaj automatycznie” była włączona.
    pub fn poll(&mut self) -> Option<Vec<PathBuf>> {
        let job = self.job.as_ref()?;
        let (new, scanned, done, truncated) = {
            let mut g = job.shared.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut g.found), g.scanned, g.done, g.truncated)
        };
        self.scanned = scanned;
        self.truncated = truncated;
        if !new.is_empty() {
            let ql: Vec<char> = self.query.chars().flat_map(char::to_lowercase).collect();
            for (path, is_dir, size) in new {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let rel_dir = path
                    .parent()
                    .and_then(|p| p.strip_prefix(&self.root).ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                let (mstart, mlen) = find_ci(&name, &ql).unwrap_or((0, 0));
                self.hits.push(Hit { path, rel_dir, name, is_dir, size, mstart, mlen, variant: 0 });
            }
            self.hits.sort_by(|a, b| (!a.is_dir, &a.rel_dir, &a.name).cmp(&(!b.is_dir, &b.rel_dir, &b.name)));
            self.marked.clear(); // indeksy po sortowaniu się zmieniły (zaznacza się zwykle po zakończeniu)
            self.assign_variants();
            if self.state.selected().is_none() {
                self.state.select(Some(0));
            }
        }
        if !done {
            return None;
        }
        self.job = None;
        self.searching = false;
        if std::mem::take(&mut self.auto_pending) {
            let v: Vec<PathBuf> = self.hits.iter().filter(|h| h.is_video()).map(|h| h.path.clone()).collect();
            return Some(v);
        }
        None
    }

    /// Nazwy różniące się tylko wielkością liter (np. Film.mkv i film.mkv) — każda pisownia
    /// dostaje własny numer koloru. Na Windows (NTFS bez rozróżniania wielkości liter) takie pliki
    /// mogą leżeć tylko w różnych katalogach; na Linuksie także w jednym.
    fn assign_variants(&mut self) {
        let mut groups: HashMap<String, BTreeSet<&str>> = HashMap::new();
        for h in &self.hits {
            groups.entry(h.name.to_lowercase()).or_default().insert(h.name.as_str());
        }
        let idx: HashMap<String, usize> = groups
            .values()
            .filter(|g| g.len() > 1)
            .flat_map(|g| g.iter().enumerate().map(|(i, n)| (n.to_string(), i + 1)))
            .collect();
        for h in self.hits.iter_mut() {
            h.variant = idx.get(&h.name).copied().unwrap_or(0);
        }
    }

    pub fn selected(&self) -> Option<&Hit> {
        self.state.selected().and_then(|i| self.hits.get(i))
    }

    pub fn move_by(&mut self, d: isize) {
        if self.hits.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let n = (cur + d).clamp(0, self.hits.len() as isize - 1);
        self.state.select(Some(n as usize));
    }

    /// Shift+ruch: zaznacza pliki po drodze (jak w lewym panelu).
    pub fn select_move(&mut self, d: isize) {
        let from = self.state.selected().unwrap_or(0);
        self.move_by(d);
        let to = self.state.selected().unwrap_or(0);
        let (a, b) = if from <= to { (from, to) } else { (to, from) };
        for i in a..=b {
            if self.hits.get(i).is_some_and(|h| !h.is_dir) {
                self.marked.insert(i);
            }
        }
    }

    pub fn toggle_here(&mut self) {
        if let Some(i) = self.state.selected() {
            if self.hits.get(i).is_some_and(|h| !h.is_dir) && !self.marked.remove(&i) {
                self.marked.insert(i);
            }
        }
    }

    /// '+' wszystkie pliki wideo, '-' żadnego, '*' odwróć (wśród plików wideo).
    pub fn marks(&mut self, op: char) {
        let videos: Vec<usize> = (0..self.hits.len()).filter(|i| self.hits[*i].is_video()).collect();
        match op {
            '+' => self.marked.extend(videos),
            '-' => self.marked.clear(),
            _ => {
                for i in videos {
                    if !self.marked.remove(&i) {
                        self.marked.insert(i);
                    }
                }
            }
        }
    }

    /// Zaznaczone (albo bieżący) pliki do kolejki — tylko wideo. Zwraca (do dodania, pominięte).
    pub fn take_for_queue(&mut self) -> (Vec<PathBuf>, usize) {
        let idx: Vec<usize> = if self.marked.is_empty() {
            self.state.selected().into_iter().collect()
        } else {
            std::mem::take(&mut self.marked).into_iter().collect()
        };
        let (mut add, mut skip) = (vec![], 0);
        for i in idx {
            match self.hits.get(i) {
                Some(h) if h.is_video() => add.push(h.path.clone()),
                Some(_) => skip += 1,
                None => {}
            }
        }
        (add, skip)
    }

    /// Wiersz listy wyników → indeks (mysz).
    pub fn index_at(&self, row: usize) -> Option<usize> {
        let i = self.state.offset() + row;
        (i < self.hits.len()).then_some(i)
    }
}

/// Dopasowanie bez rozróżniania wielkości liter: (indeks znaku, długość w znakach).
fn find_ci(name: &str, ql: &[char]) -> Option<(usize, usize)> {
    let nl: Vec<char> = name.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect();
    if ql.is_empty() || ql.len() > nl.len() {
        return None;
    }
    (0..=nl.len() - ql.len()).find(|&i| nl[i..i + ql.len()] == *ql).map(|i| (i, ql.len()))
}

/// Systemowe katalogi wirtualne pomijane przy szukaniu od korzenia (Linux).
fn skip_dir(p: &Path) -> bool {
    cfg!(target_os = "linux") && ["/proc", "/sys", "/dev", "/run"].iter().any(|s| p == Path::new(s))
}

fn search(root: &Path, q: &str, recursive: bool, shared: &Mutex<Shared>, cancel: &AtomicBool) {
    let ql: Vec<char> = q.chars().collect();
    let mut stack = vec![root.to_path_buf()];
    let mut batch: Vec<(PathBuf, bool, u64)> = vec![];
    let (mut scanned, mut total) = (0usize, 0usize);
    let flush = |batch: &mut Vec<(PathBuf, bool, u64)>, scanned: usize, done: bool, truncated: bool| {
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.found.append(batch);
        g.scanned = scanned;
        g.done = done;
        g.truncated |= truncated;
    };
    let mut last = Instant::now();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            scanned += 1;
            // Bez podążania za dowiązaniami do katalogów (pętle, inne dyski).
            let Ok(ft) = e.file_type() else { continue };
            let path = e.path();
            let is_dir = ft.is_dir() || (ft.is_symlink() && path.is_dir());
            if ft.is_dir() && recursive && !skip_dir(&path) {
                stack.push(path.clone());
            }
            let name = e.file_name().to_string_lossy().to_string();
            if find_ci(&name, &ql).is_some() {
                if total >= MAX_HITS {
                    flush(&mut batch, scanned, true, true);
                    return;
                }
                let size = if is_dir { 0 } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
                batch.push((path, is_dir, size));
                total += 1;
            }
            if last.elapsed().as_millis() >= 100 {
                flush(&mut batch, scanned, false, false);
                last = Instant::now();
            }
        }
    }
    flush(&mut batch, scanned, true, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_and_variants() {
        let q: Vec<char> = "fil".chars().collect();
        assert_eq!(find_ci("Mój FILM.mkv", &q), Some((4, 3)));
        assert_eq!(find_ci("abc", &q), None);

        let d = std::env::temp_dir().join(format!("vp-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        for f in ["Film.mkv", "a/film.mkv", "a/b/FILM.txt", "a/inny.mp4"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let mut v = FindView::new(d.clone());
        v.input = TextInput { chars: "film".chars().collect(), cursor: 4 };
        v.auto_add = true;
        v.start();
        let mut added = None;
        for _ in 0..100 {
            if let Some(a) = v.poll() {
                added = Some(a);
            }
            if !v.searching {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let names: Vec<(&str, usize)> = v.hits.iter().map(|h| (h.name.as_str(), h.variant)).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        // Film.mkv i film.mkv — ta sama nazwa w innej pisowni: różne kolory; FILM.txt to inna nazwa.
        assert_eq!(names, [("Film.mkv", 1), ("film.mkv", 2), ("FILM.txt", 0)]);
        assert_eq!(added.map(|a| a.len()), Some(2)); // tylko .mkv
        v.recursive = false;
        v.start();
        while v.searching {
            v.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(v.hits.len(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
