//! Stan aplikacji TUI i obsługa klawiszy (styl Midnight Commandera).
use crate::{
    browser::{self, AddResult, Browser, Queue},
    render,
};
use videopipeline_core::{
    config::*,
    hw::HwInfo,
    log,
    model::{Panel, Snapshot, State},
    pipeline::{self, RunStatus, SharedStatus},
    proc,
    sysmon::{Monitor, SysStats},
};
use anyhow::Result;
use ratatui::{
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
            MouseButton, MouseEvent, MouseEventKind,
        },
        execute,
    },
    layout::Rect,
    DefaultTerminal,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Options,
    Help,
    Running,
    Finished,
    ConfirmAbort,
    ConfirmQuit,
    /// F6: potwierdzenie usunięcia plików z dysku
    ConfirmDelete,
    /// F4: zmiana nazwy
    Rename,
    /// F2: informacje MediaInfo o pliku
    MediaInfo,
    /// Okienko z komunikatem (np. dlaczego nie da się usunąć pliku)
    Message,
}

/// Okienko F2: raport MediaInfo (albo błąd) i przewinięcie.
pub struct MediaInfoView {
    pub file: String,
    pub report: Result<Vec<videopipeline_core::mediainfo::Section>, String>,
    pub scroll: usize,
    /// Ustawiane przy rysowaniu: maksymalne przewinięcie.
    pub max_scroll: usize,
}

/// Położenie elementów na ekranie z ostatniego rysowania (dla myszy).
#[derive(Default, Clone, Copy)]
pub struct UiRects {
    pub left_list: Rect,
    pub right_list: Rect,
    pub fkeys: Rect,
}

/// Przewijanie długiej nazwy zaznaczonej pozycji: (klucz pozycji, od kiedy zaznaczona).
pub struct Marquee {
    pub key: String,
    pub since: Instant,
}

/// F6: ile katalogów jest wśród usuwanych pozycji i co w nich jest.
#[derive(Default, Clone, Copy)]
pub struct DeleteInfo {
    pub dirs: usize,
    /// Pliki wewnątrz katalogów (rekurencyjnie)
    pub inner_files: u64,
    /// Łączny rozmiar plików wewnątrz katalogów
    pub inner_bytes: u64,
    /// Liczenie przerwane po limicie (bardzo duże drzewo) — liczby są dolną granicą
    pub truncated: bool,
}

/// Zlicza pliki i rozmiar w katalogu (bez wchodzenia w dowiązania symboliczne), z limitem wpisów.
fn tally_dir(dir: &Path, info: &mut DeleteInfo, budget: &mut u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *budget == 0 {
            info.truncated = true;
            return;
        }
        *budget -= 1;
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        if md.is_dir() {
            tally_dir(&e.path(), info, budget);
        } else {
            info.inner_files += 1;
            info.inner_bytes += md.len();
        }
    }
}

/// Katalogi, których F6 nigdy nie usuwa (korzeń systemu, dysku, katalog domowy).
fn is_protected_dir(p: &Path) -> bool {
    p.parent().is_none() || videopipeline_core::platform::home_dir().is_some_and(|h| h == p)
}

/// Usuwa plik, dowiązanie albo katalog z całą zawartością (dowiązania usuwane są jako dowiązania).
fn remove_entry(p: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(p)?;
    if md.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// Pole tekstowe z kursorem (zmiana nazwy).
pub struct TextInput {
    pub chars: Vec<char>,
    pub cursor: usize,
}

impl TextInput {
    /// Kursor przed rozszerzeniem — najczęściej dopisuje się coś do nazwy (np. "_v2").
    pub fn for_filename(name: &str) -> Self {
        let chars: Vec<char> = name.chars().collect();
        let cursor = match name.rfind('.') {
            Some(i) if i > 0 => name[..i].chars().count(),
            _ => chars.len(),
        };
        TextInput { chars, cursor }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// true = klawisz obsłużony (edycja albo ruch kursora).
    fn key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char(c) => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }
}

pub struct RenameCtx {
    pub from: PathBuf,
    pub input: TextInput,
}

pub const OPTION_COUNT: usize = 13;
/// Indeksy wierszy w oknie opcji (F7).
pub const OPT_HEVC_CQ: usize = 1;
pub const OPT_AV1_CQ: usize = 2;
pub const OPT_QSV_PRESET: usize = 3;

pub struct RunCtx {
    pub status: SharedStatus,
    pub panel: Panel,
    pub handle: Option<JoinHandle<()>>,
    pub opts: Opts,
    pub last: RunStatus,
}

pub struct App {
    pub opts: Opts,
    /// Wykryte enkodery GPU (przy starcie).
    pub hw: HwInfo,
    pub browser: Browser,
    pub queue: Queue,
    pub focus: Focus,
    pub mode: Mode,
    pub opt_cursor: usize,
    pub opt_edit: Option<String>,
    pub status_msg: String,
    pub run: Option<RunCtx>,
    pub snapshot: Snapshot,
    pub shutdown_at: Option<Instant>,
    pub quit: bool,
    pub do_shutdown: bool,
    /// Obciążenie GPU/CPU/VRAM — monitor działa tylko podczas kodowania.
    pub sys: SysStats,
    pub monitor: Option<Monitor>,
    /// F6: pliki czekające na potwierdzenie usunięcia
    pub pending_delete: Vec<PathBuf>,
    /// F6: podsumowanie zawartości usuwanych katalogów (do ostrzeżenia)
    pub pending_delete_info: DeleteInfo,
    /// F4: trwająca zmiana nazwy
    pub rename: Option<RenameCtx>,
    /// F2: okienko MediaInfo
    pub mediainfo: Option<MediaInfoView>,
    /// Ostatni układ ekranu (obsługa myszy)
    pub ui: UiRects,
    /// Ostatnie kliknięcie: panel, wiersz, czas — do wykrywania podwójnego kliknięcia
    last_click: Option<(Focus, usize, Instant)>,
    /// Przewijanie długiej nazwy zaznaczonego pliku
    pub marquee: Marquee,
    /// Ctrl+L: wymuś pełne przerysowanie ekranu
    pub force_redraw: bool,
    /// Okienko komunikatu: tytuł i linie
    pub message: Option<(String, Vec<String>)>,
}

impl App {
    pub fn new(opts: Opts, hw: HwInfo, start: PathBuf, initial: Vec<PathBuf>) -> Self {
        let mut app = App {
            opts,
            hw,
            browser: Browser::new(start),
            queue: Queue::new(),
            focus: Focus::Left,
            mode: Mode::Normal,
            opt_cursor: 0,
            opt_edit: None,
            status_msg: String::new(),
            run: None,
            snapshot: Snapshot::default(),
            shutdown_at: None,
            quit: false,
            do_shutdown: false,
            sys: SysStats::default(),
            monitor: None,
            pending_delete: vec![],
            pending_delete_info: DeleteInfo::default(),
            rename: None,
            mediainfo: None,
            ui: UiRects::default(),
            last_click: None,
            marquee: Marquee { key: String::new(), since: Instant::now() },
            force_redraw: false,
            message: None,
        };
        if !initial.is_empty() {
            app.add_paths(initial);
        }
        app
    }

    // ------------------------------------------------------------------ run ---

    /// Odświeża migawkę stanu pipeline'u i wykrywa zakończenie pracy.
    pub fn poll_run(&mut self) {
        let Some(run) = self.run.as_mut() else { return };
        self.snapshot = run.panel.snapshot();
        let st = run.status.lock().map(|g| g.clone()).unwrap_or(RunStatus::Running);
        if st != RunStatus::Running && run.last == RunStatus::Running {
            run.last = st.clone();
            if let Some(h) = run.handle.take() {
                let _ = h.join();
            }
            if st == RunStatus::Done && run.opts.shutdown {
                self.shutdown_at = Some(Instant::now() + Duration::from_secs(10));
            }
            self.monitor = None; // koniec pracy — zatrzymaj nvidia-smi
            self.mode = Mode::Finished;
        }
    }

    pub fn check_shutdown(&mut self) {
        if let Some(t) = self.shutdown_at {
            if Instant::now() >= t {
                self.do_shutdown = true;
                self.quit = true;
            }
        }
    }

    fn start_run(&mut self) {
        if self.queue.items.is_empty() {
            self.status_msg = "Kolejka jest pusta — zaznacz pliki w lewym panelu i naciśnij F5".into();
            return;
        }
        if !self.hw.has(self.opts.engine, self.opts.codec) {
            self.status_msg = format!(
                "{} nie działa w tym systemie (wykryte: {}) — zmień kodek w F7",
                self.opts.codec_label(),
                self.hw.summary()
            );
            return;
        }
        proc::set_cancel(false);
        log::clear();
        let files: Vec<PathBuf> = self.queue.items.iter().map(|q| q.path.clone()).collect();
        let panel = Panel::default();
        let status: SharedStatus = Arc::new(Mutex::new(RunStatus::Running));
        let handle = pipeline::spawn(self.opts.clone(), files, panel.clone(), status.clone());
        self.run = Some(RunCtx {
            status,
            panel,
            handle: Some(handle),
            opts: self.opts.clone(),
            last: RunStatus::Running,
        });
        self.snapshot = Snapshot::default();
        self.shutdown_at = None;
        self.sys = SysStats::default();
        self.monitor = Some(Monitor::start(self.opts.engine, self.hw.intel_dev.as_deref()));
        self.mode = Mode::Running;
    }

    fn abort(&mut self) {
        proc::set_cancel(true);
        proc::kill_all();
    }

    fn close_run(&mut self) {
        // Z kolejki znikają pliki, które zostały w pełni przetworzone.
        let done: Vec<String> = self
            .snapshot
            .items
            .iter()
            .filter(|(_, it)| it.state == State::Done)
            .map(|(n, _)| n.clone())
            .collect();
        self.queue.retain_by_name(|n| !done.iter().any(|d| d == n));
        self.run = None;
        self.snapshot = Snapshot::default();
        self.shutdown_at = None;
        self.monitor = None;
        self.mode = Mode::Normal;
        proc::set_cancel(false);
    }

    fn request_quit(&mut self) {
        if matches!(self.mode, Mode::Running | Mode::ConfirmAbort | Mode::ConfirmQuit) {
            self.mode = Mode::ConfirmQuit;
        } else {
            self.quit = true;
        }
    }

    // ----------------------------------------------------------------- queue ---

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let (mut added, mut dup, mut clash) = (0, 0, 0);
        for p in paths {
            let list = if p.is_dir() {
                browser::videos_in(&p)
            } else if browser::is_video(&p) {
                vec![p]
            } else {
                vec![]
            };
            for v in list {
                match self.queue.add(v) {
                    AddResult::Added => added += 1,
                    AddResult::Duplicate => dup += 1,
                    AddResult::NameClash => clash += 1,
                }
            }
        }
        self.status_msg = format!("Dodano do kolejki: {added}");
        if dup > 0 {
            self.status_msg.push_str(&format!(", już były: {dup}"));
        }
        if clash > 0 {
            self.status_msg.push_str(&format!(", pominięto (ta sama nazwa pliku): {clash}"));
        }
    }

    fn add_from_left(&mut self) {
        let mut paths: Vec<PathBuf> = vec![];
        if !self.browser.marked.is_empty() {
            paths.extend(self.browser.marked.iter().cloned());
            self.browser.marked.clear();
        } else if let Some(e) = self.browser.selected() {
            if !e.is_up {
                paths.push(e.path.clone());
            }
        }
        self.add_paths(paths);
    }

    fn move_cursor(&mut self, d: isize) {
        match self.focus {
            Focus::Left => self.browser.move_by(d),
            Focus::Right => self.queue.move_by(d),
        }
    }

    /// Shift+ruch — zaznaczanie zakresu jak w Midnight Commanderze.
    fn select_move(&mut self, d: isize) {
        match self.focus {
            Focus::Left => self.browser.select_move(d),
            Focus::Right => self.queue.select_move(d),
        }
    }

    /// '+' wszystko, '-' wyczyść, '*' odwróć.
    fn marks(&mut self, op: char) {
        match (self.focus, op) {
            (Focus::Left, '+') => self.browser.mark_all(),
            (Focus::Left, '-') => self.browser.clear_marks(),
            (Focus::Left, _) => self.browser.invert_marks(),
            (Focus::Right, '+') => self.queue.mark_all(),
            (Focus::Right, '-') => self.queue.clear_marks(),
            (Focus::Right, _) => self.queue.invert_marks(),
        }
    }

    fn page(&self) -> isize {
        match self.focus {
            Focus::Left => self.browser.page.max(1) as isize,
            Focus::Right => self.queue.page.max(1) as isize,
        }
    }

    // ------------------------------------------------------------------ keys ---

    pub fn on_key(&mut self, k: KeyEvent) {
        self.status_msg.clear();
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.request_quit();
            return;
        }
        // Ctrl+L — przerysuj ekran (jak w Midnight Commanderze).
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('l') {
            self.force_redraw = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.key_normal(k),
            Mode::Options => self.key_options(k),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Running => {
                if matches!(k.code, KeyCode::Esc | KeyCode::F(10)) {
                    self.mode = Mode::ConfirmAbort;
                }
            }
            Mode::ConfirmAbort => self.key_confirm(k, false),
            Mode::ConfirmQuit => self.key_confirm(k, true),
            Mode::ConfirmDelete => self.key_confirm_delete(k),
            Mode::Rename => self.key_rename(k),
            Mode::MediaInfo => self.key_mediainfo(k),
            Mode::Message => {
                if matches!(k.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') | KeyCode::F(10)) {
                    self.message = None;
                    self.mode = Mode::Normal;
                }
            }
            Mode::Finished => {
                if self.shutdown_at.is_some() {
                    self.shutdown_at = None;
                    self.status_msg = "Wyłączenie komputera anulowane".into();
                } else {
                    self.close_run();
                }
            }
        }
    }

    fn key_normal(&mut self, k: KeyEvent) {
        // Shift + strzałki/PgUp/PgDn/Home/End: zaznaczanie wielu plików (jak w MC).
        if k.modifiers.contains(KeyModifiers::SHIFT) {
            let page = self.page();
            let d = match k.code {
                KeyCode::Up => Some(-1),
                KeyCode::Down => Some(1),
                KeyCode::PageUp => Some(-page),
                KeyCode::PageDown => Some(page),
                KeyCode::Home => Some(-1_000_000),
                KeyCode::End => Some(1_000_000),
                _ => None,
            };
            if let Some(d) = d {
                self.select_move(d);
                return;
            }
        }
        match k.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Left => Focus::Right,
                    Focus::Right => Focus::Left,
                }
            }
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::PageUp => {
                let p = self.page();
                self.move_cursor(-p);
            }
            KeyCode::PageDown => {
                let p = self.page();
                self.move_cursor(p);
            }
            KeyCode::Home => self.move_cursor(-1_000_000),
            KeyCode::End => self.move_cursor(1_000_000),
            KeyCode::Enter => {
                if self.focus == Focus::Left {
                    if let Some(p) = self.browser.activate() {
                        self.add_paths(vec![p]);
                    }
                }
            }
            KeyCode::Backspace => {
                if self.focus == Focus::Left {
                    self.browser.go_up();
                }
            }
            KeyCode::Insert | KeyCode::Char(' ') => match self.focus {
                Focus::Left => self.browser.toggle_mark(),
                Focus::Right => self.queue.toggle_mark(),
            },
            KeyCode::Char(c @ ('+' | '-' | '*')) => self.marks(c),
            KeyCode::F(1) => self.mode = Mode::Help,
            KeyCode::F(2) => self.begin_mediainfo(),
            KeyCode::F(3) => self.play_selected(),
            KeyCode::F(4) => self.begin_rename(),
            KeyCode::F(5) => self.add_from_left(),
            KeyCode::F(6) => self.begin_delete(),
            KeyCode::F(7) => self.mode = Mode::Options,
            KeyCode::F(8) | KeyCode::Delete => {
                let n = self.queue.remove_marked_or_current();
                self.status_msg = format!("Usunięto z kolejki: {n}");
            }
            KeyCode::F(9) => self.start_run(),
            KeyCode::F(10) => self.request_quit(),
            _ => {}
        }
    }

    /// Czy zaznaczona nazwa jest właśnie przewijana (ustawiane przy rysowaniu).
    pub fn marquee_active(&self) -> bool {
        !self.marquee.key.is_empty()
    }

    /// F3: odtwórz bieżący plik (lewy panel) albo pozycję kolejki (prawy panel) w mpv.
    fn play_selected(&mut self) {
        let path = match self.focus {
            Focus::Left => self.browser.selected().filter(|e| !e.is_dir && !e.is_up).map(|e| e.path.clone()),
            Focus::Right => self.queue.state.selected().and_then(|i| self.queue.items.get(i)).map(|q| q.path.clone()),
        };
        let Some(path) = path else {
            self.status_msg = "F3 odtwarza plik w mpv — wybierz plik (nie katalog)".into();
            return;
        };
        self.status_msg = match crate::player::play(&path) {
            Ok(m) | Err(m) => m,
        };
    }

    // ------------------------------------------------------------ MediaInfo (F2) ---

    /// F2: informacje o bieżącym pliku (lewy panel) albo pozycji kolejki (prawy panel).
    fn begin_mediainfo(&mut self) {
        let path = match self.focus {
            Focus::Left => self.browser.selected().filter(|e| !e.is_dir && !e.is_up).map(|e| e.path.clone()),
            Focus::Right => self.queue.state.selected().and_then(|i| self.queue.items.get(i)).map(|q| q.path.clone()),
        };
        let Some(path) = path else {
            self.status_msg = "F2 pokazuje informacje o pliku — wybierz plik (nie katalog)".into();
            return;
        };
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let report = videopipeline_core::mediainfo::easy(&path);
        self.mediainfo = Some(MediaInfoView { file, report, scroll: 0, max_scroll: 0 });
        self.mode = Mode::MediaInfo;
    }

    fn key_mediainfo(&mut self, k: KeyEvent) {
        let Some(v) = self.mediainfo.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        match k.code {
            KeyCode::Esc | KeyCode::F(2) | KeyCode::F(10) | KeyCode::Enter | KeyCode::Char('q') => {
                self.mediainfo = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Up => v.scroll = v.scroll.saturating_sub(1),
            KeyCode::Down => v.scroll = (v.scroll + 1).min(v.max_scroll),
            KeyCode::PageUp => v.scroll = v.scroll.saturating_sub(10),
            KeyCode::PageDown => v.scroll = (v.scroll + 10).min(v.max_scroll),
            KeyCode::Home => v.scroll = 0,
            KeyCode::End => v.scroll = v.max_scroll,
            _ => {}
        }
    }

    // ------------------------------------------------------------------- mysz ---

    /// Kliknięcie: ustawia kursor i zaznacza/odznacza plik; podwójne kliknięcie na pliku dodaje zaznaczone + kliknięty do kolejki,
    /// na katalogu — wchodzi do niego. Prawy przycisk: MediaInfo. Kółko: przewijanie. Pasek F1–F10: klawisze.
    pub fn on_mouse(&mut self, m: MouseEvent) {
        let inside = |r: Rect| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        match self.mode {
            Mode::MediaInfo => {
                if let Some(v) = self.mediainfo.as_mut() {
                    match m.kind {
                        MouseEventKind::ScrollDown => v.scroll = (v.scroll + 3).min(v.max_scroll),
                        MouseEventKind::ScrollUp => v.scroll = v.scroll.saturating_sub(3),
                        MouseEventKind::Down(_) => {
                            self.mediainfo = None;
                            self.mode = Mode::Normal;
                        }
                        _ => {}
                    }
                }
                return;
            }
            Mode::Normal => {}
            _ => return,
        }
        let ui = self.ui;
        // Pasek klawiszy funkcyjnych.
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            if inside(ui.fkeys) && ui.fkeys.width >= 10 {
                let cell = (ui.fkeys.width / 10).max(1);
                let n = ((m.column - ui.fkeys.x) / cell).min(9) as u8 + 1;
                self.on_key(KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE));
                return;
            }
        }
        let (panel, list) = if inside(ui.left_list) {
            (Focus::Left, ui.left_list)
        } else if inside(ui.right_list) {
            (Focus::Right, ui.right_list)
        } else {
            return;
        };
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                self.focus = panel;
                self.move_cursor(d);
            }
            MouseEventKind::Down(button) => {
                self.status_msg.clear();
                self.focus = panel;
                let row = (m.row - list.y) as usize;
                let (offset, len) = match panel {
                    Focus::Left => (self.browser.state.offset(), self.browser.entries.len()),
                    Focus::Right => (self.queue.state.offset(), self.queue.items.len()),
                };
                let idx = offset + row;
                if idx >= len {
                    return;
                }
                match panel {
                    Focus::Left => self.browser.state.select(Some(idx)),
                    Focus::Right => self.queue.state.select(Some(idx)),
                }
                if button == MouseButton::Right {
                    self.begin_mediainfo();
                    return;
                }
                if button != MouseButton::Left {
                    return;
                }
                let now = Instant::now();
                let double = matches!(self.last_click,
                    Some((p, i, t)) if p == panel && i == idx && now.duration_since(t) < Duration::from_millis(450));
                self.last_click = if double { None } else { Some((panel, idx, now)) };
                // Pojedyncze kliknięcie na pliku w lewym panelu zaznacza go / odznacza (jak Spacja).
                if !double && panel == Focus::Left && self.browser.selected().is_some_and(|e| !e.is_dir && !e.is_up) {
                    self.browser.toggle_mark_here();
                }
                if double && panel == Focus::Left {
                    let (is_file, path) = match self.browser.selected() {
                        Some(e) => (!e.is_dir && !e.is_up, e.path.clone()),
                        None => return,
                    };
                    if is_file {
                        self.browser.marked.insert(path); // zaznaczone + kliknięty
                        self.add_from_left();
                    } else if let Some(p) = self.browser.activate() {
                        self.add_paths(vec![p]);
                    }
                }
            }
            _ => {}
        }
    }

    // --------------------------------------------------------- operacje na plikach ---

    /// F6: usuń zaznaczone (lub bieżący) pliki z dysku — tylko lewy panel, tylko pliki.
    fn begin_delete(&mut self) {
        if self.focus != Focus::Left {
            self.status_msg = "F6 usuwa pliki z dysku w lewym panelu (z kolejki usuwa F8)".into();
            return;
        }
        let targets = self.browser.targets();
        if targets.is_empty() {
            self.status_msg = "Nic nie wybrano".into();
            return;
        }
        if let Some(p) = targets.iter().find(|p| is_protected_dir(p)) {
            self.status_msg = format!("{} nie zostanie usunięty (katalog główny lub domowy)", p.display());
            return;
        }
        // Katalogi: policz zawartość, żeby ostrzeżenie mówiło, ile naprawdę zniknie.
        let mut info = DeleteInfo::default();
        let mut budget: u32 = 200_000;
        for p in &targets {
            let is_dir = std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
            if is_dir {
                info.dirs += 1;
                tally_dir(p, &mut info, &mut budget);
            }
        }
        self.pending_delete = targets;
        self.pending_delete_info = info;
        self.mode = Mode::ConfirmDelete;
    }

    fn key_confirm_delete(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('y' | 'Y' | 't' | 'T') | KeyCode::Enter => {
                let files = std::mem::take(&mut self.pending_delete);
                let mut removed: Vec<PathBuf> = vec![];
                let mut errors: Vec<(PathBuf, std::io::Error)> = vec![];
                for f in &files {
                    match remove_entry(f) {
                        Ok(()) => removed.push(f.clone()),
                        Err(e) => errors.push((f.clone(), e)),
                    }
                }
                self.queue.remove_paths(&removed);
                self.browser.marked.retain(|m| !removed.iter().any(|d| m.starts_with(d)));
                self.browser.reload(None);
                self.mode = Mode::Normal;
                if errors.is_empty() {
                    self.status_msg = format!("Usunięto z dysku: {}", removed.len());
                } else {
                    self.status_msg = format!("Usunięto: {}, nie udało się: {}", removed.len(), errors.len());
                    // Okienko z wyjaśnieniem: czego brakuje i jak to naprawić (do 3 pozycji).
                    let mut lines = vec![];
                    for (i, (p, e)) in errors.iter().take(3).enumerate() {
                        if i > 0 {
                            lines.push(String::new());
                        }
                        lines.extend(crate::perm::explain(p, e));
                    }
                    if errors.len() > 3 {
                        lines.push(String::new());
                        lines.push(format!("… i {} kolejnych błędów.", errors.len() - 3));
                    }
                    self.message = Some(("Nie udało się usunąć".into(), lines));
                    self.mode = Mode::Message;
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.pending_delete.clear();
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    /// F4: zmiana nazwy bieżącego pliku lub katalogu w lewym panelu.
    fn begin_rename(&mut self) {
        if self.focus != Focus::Left {
            self.status_msg = "F4 zmienia nazwę w lewym panelu".into();
            return;
        }
        let Some(e) = self.browser.selected().filter(|e| !e.is_up) else {
            self.status_msg = "Nic nie wybrano".into();
            return;
        };
        self.rename = Some(RenameCtx { from: e.path.clone(), input: TextInput::for_filename(&e.name) });
        self.mode = Mode::Rename;
    }

    fn key_rename(&mut self, k: KeyEvent) {
        let Some(ctx) = self.rename.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        match k.code {
            KeyCode::Esc => {
                self.rename = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let name = ctx.input.text().trim().to_string();
                let from = ctx.from.clone();
                let old_name = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if name.is_empty() || name == "." || name == ".." || name.contains('/') || (cfg!(windows) && name.contains(['\\', ':'])) {
                    self.status_msg = "Niepoprawna nazwa (pusta, \".\", \"..\" albo z \"/\")".into();
                    return;
                }
                if name == old_name {
                    self.rename = None;
                    self.mode = Mode::Normal;
                    return;
                }
                let to = from.with_file_name(&name);
                if to.exists() {
                    self.status_msg = format!("{name} już istnieje — podaj inną nazwę");
                    return;
                }
                match std::fs::rename(&from, &to) {
                    Ok(()) => {
                        self.queue.rename_path(&from, &to);
                        if self.browser.marked.remove(&from) {
                            self.browser.marked.insert(to.clone());
                        }
                        // Plik, który przestał wyglądać na film (inne rozszerzenie), zniknie z panelu.
                        self.browser.reload(Some(&name));
                        self.status_msg = format!("{old_name} → {name}");
                    }
                    Err(e) => self.status_msg = format!("Nie udało się zmienić nazwy: {e}"),
                }
                self.rename = None;
                self.mode = Mode::Normal;
            }
            code => {
                ctx.input.key(code);
            }
        }
    }

    fn key_confirm(&mut self, k: KeyEvent, quit_after: bool) {
        match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Enter => {
                self.abort();
                if quit_after {
                    if let Some(run) = self.run.as_mut() {
                        if let Some(h) = run.handle.take() {
                            let _ = h.join();
                        }
                    }
                    self.quit = true;
                } else {
                    self.mode = Mode::Running;
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.mode = Mode::Running,
            _ => {}
        }
    }

    fn key_options(&mut self, k: KeyEvent) {
        // Tryb edycji katalogu wyjściowego.
        if let Some(mut buf) = self.opt_edit.take() {
            match k.code {
                KeyCode::Enter => {
                    let v = buf.trim().to_string();
                    if !v.is_empty() {
                        self.opts.output_dir = expand_home(&v);
                    }
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    buf.pop();
                    self.opt_edit = Some(buf);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    self.opt_edit = Some(buf);
                }
                _ => self.opt_edit = Some(buf),
            }
            return;
        }

        match k.code {
            KeyCode::Esc | KeyCode::F(7) | KeyCode::F(10) => self.mode = Mode::Normal,
            KeyCode::Up => self.opt_cursor = self.opt_cursor.saturating_sub(1),
            KeyCode::Down => self.opt_cursor = (self.opt_cursor + 1).min(OPTION_COUNT - 1),
            KeyCode::Left | KeyCode::Char('-') => self.adjust_option(-1),
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_option(1),
            KeyCode::PageDown => self.adjust_option(-5),
            KeyCode::PageUp => self.adjust_option(5),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle_option(),
            _ => {}
        }
    }

    /// Strzałki: na wierszach CQ zmieniają wartość o `d`, na pozostałych przełączają opcję.
    fn adjust_option(&mut self, d: i32) {
        let bump = |v: u8| (v as i32 + d).clamp(CQ_MIN as i32, CQ_MAX as i32) as u8;
        match self.opt_cursor {
            OPT_HEVC_CQ => self.opts.hevc_cq = bump(self.opts.hevc_cq),
            OPT_AV1_CQ => self.opts.av1_cq = bump(self.opts.av1_cq),
            OPT_QSV_PRESET => self.cycle_preset(d.signum()),
            0 => self.cycle_codec(if d < 0 { -1 } else { 1 }),
            _ => self.toggle_option(),
        }
    }

    /// Kodek + silnik: przełącza między działającymi kombinacjami (np. HEVC NVENC → AV1 NVENC → HEVC QSV).
    fn cycle_codec(&mut self, d: i32) {
        let combos = self.hw.combos();
        if combos.is_empty() {
            // Bez wykrytych enkoderów: przynajmniej przełącz kodek.
            self.opts.codec = match self.opts.codec {
                Codec::Hevc => Codec::Av1,
                Codec::Av1 => Codec::Hevc,
            };
            return;
        }
        let n = combos.len() as i32;
        let next = match combos.iter().position(|c| *c == (self.opts.engine, self.opts.codec)) {
            Some(i) => (i as i32 + d).rem_euclid(n),
            None => 0,
        };
        let (e, c) = combos[next as usize];
        self.opts.engine = e;
        self.opts.codec = c;
    }

    /// Preset QSV: ← szybciej, → wolniej i lepiej (bez zawijania).
    fn cycle_preset(&mut self, d: i32) {
        let i = QSV_PRESETS.iter().position(|p| *p == self.opts.qsv_preset).unwrap_or(4) as i32;
        let n = (i + d).clamp(0, QSV_PRESETS.len() as i32 - 1);
        self.opts.qsv_preset = QSV_PRESETS[n as usize];
    }

    fn toggle_option(&mut self) {
        match self.opt_cursor {
            0 => self.cycle_codec(1),
            // 1, 2: wiersze CQ — zmieniane strzałkami (Enter/Spacja nic nie robią)
            // 3: preset QSV — Enter/Spacja przechodzą do następnego (wolniejszego), na końcu wraca na początek
            OPT_QSV_PRESET => {
                let i = QSV_PRESETS.iter().position(|p| *p == self.opts.qsv_preset).unwrap_or(4);
                self.opts.qsv_preset = QSV_PRESETS[(i + 1) % QSV_PRESETS.len()];
            }
            4 => self.opts.pl_eac3 = !self.opts.pl_eac3,
            5 => self.opts.extra = !self.opts.extra,
            6 => self.opts.denoise = !self.opts.denoise,
            7 => self.opts.no_crop = !self.opts.no_crop,
            8 => self.opts.no_dv = !self.opts.no_dv,
            9 => self.opts.no_hdr10plus = !self.opts.no_hdr10plus,
            10 => self.opts.copy = !self.opts.copy,
            11 => self.opts.shutdown = !self.opts.shutdown,
            12 => self.opt_edit = Some(self.opts.output_dir.to_string_lossy().to_string()),
            _ => {}
        }
    }
}

fn expand_home(s: &str) -> PathBuf {
    videopipeline_core::platform::expand_home(s)
}

// -------------------------------------------------------------------------

pub fn run(app: &mut App) -> Result<()> {
    let _look = crate::window::Look::apply(); // tytuł „VideoPipeline” (Windows: też ikona okna)
    let mut terminal = ratatui::init();
    // Mysz: kliknięcia i kółko. (Zaznaczanie tekstu w terminalu działa wtedy z wciśniętym Shift.)
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = event_loop(&mut terminal, app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    proc::kill_all();
    res
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_tick = Instant::now();
    let mut last_mode = app.mode;
    while !app.quit {
        app.poll_run();
        // Zamknięcie okienka (F2, F4, F6, F7, F9…) albo Ctrl+L: pełne przerysowanie ekranu.
        // Część terminali (np. Konsole/Yakuake) potrafi zostawić resztki okna, gdy ratatui
        // odświeża tylko zmienione komórki — czyszczenie wymusza narysowanie wszystkiego od nowa.
        if app.mode != last_mode || app.force_redraw {
            terminal.clear()?;
            app.force_redraw = false;
            last_mode = app.mode;
        }
        terminal.draw(|f| render::draw(f, app))?;

        // Krótszy odstęp, gdy przewija się długa nazwa (płynniejszy ruch tekstu).
        let wait = if app.marquee_active() { 120 } else { 200 };
        if event::poll(Duration::from_millis(wait))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                Event::Mouse(m) => app.on_mouse(m),
                _ => {}
            }
        }
        if last_tick.elapsed() >= Duration::from_secs(1) {
            if let Some(r) = &app.run {
                r.panel.tick();
            }
            if let Some(m) = app.monitor.as_mut() {
                app.sys = m.sample();
            }
            last_tick = Instant::now();
        }
        app.check_shutdown();
    }
    Ok(())
}
