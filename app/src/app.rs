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
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    DefaultTerminal,
};
use std::{
    path::PathBuf,
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
    /// F4: trwająca zmiana nazwy
    pub rename: Option<RenameCtx>,
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
            rename: None,
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

    // --------------------------------------------------------- operacje na plikach ---

    /// F6: usuń zaznaczone (lub bieżący) pliki z dysku — tylko lewy panel, tylko pliki.
    fn begin_delete(&mut self) {
        if self.focus != Focus::Left {
            self.status_msg = "F6 usuwa pliki z dysku w lewym panelu (z kolejki usuwa F8)".into();
            return;
        }
        let targets = self.browser.targets();
        let files: Vec<PathBuf> = targets.iter().filter(|p| p.is_file()).cloned().collect();
        if files.is_empty() {
            self.status_msg = if targets.is_empty() {
                "Nic nie wybrano".into()
            } else {
                "Katalogi nie są usuwane — zaznacz pliki".into()
            };
            return;
        }
        self.pending_delete = files;
        self.mode = Mode::ConfirmDelete;
    }

    fn key_confirm_delete(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('y' | 'Y' | 't' | 'T') | KeyCode::Enter => {
                let files = std::mem::take(&mut self.pending_delete);
                let mut removed: Vec<PathBuf> = vec![];
                let mut errors: Vec<String> = vec![];
                for f in &files {
                    match std::fs::remove_file(f) {
                        Ok(()) => removed.push(f.clone()),
                        Err(e) => errors.push(format!(
                            "{}: {e}",
                            f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                        )),
                    }
                }
                self.queue.remove_paths(&removed);
                for p in &removed {
                    self.browser.marked.remove(p);
                }
                self.browser.reload(None);
                self.status_msg = match errors.first() {
                    None => format!("Usunięto z dysku: {}", removed.len()),
                    Some(e) => format!("Usunięto: {}, błędy: {} ({e})", removed.len(), errors.len()),
                };
                self.mode = Mode::Normal;
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
                if name.is_empty() || name == "." || name == ".." || name.contains('/') {
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
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(s)
}

// -------------------------------------------------------------------------

pub fn run(app: &mut App) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, app);
    ratatui::restore();
    proc::kill_all();
    res
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_tick = Instant::now();
    while !app.quit {
        app.poll_run();
        terminal.draw(|f| render::draw(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.on_key(k);
                }
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
