//! Rysowanie TUI: dwa panele w stylu Midnight Commandera + okienka na środku.
use crate::{
    app::{App, Focus, Mode, OPTION_COUNT},
    browser::{Browser, Queue},
    theme::*,
};
use videopipeline_core::{
    config::*,
    log,
    model::{enc_progress, hms, overall_pct, Enc, Item, Snapshot, State},
    pipeline::RunStatus,
    intelmon::IntelStats,
    sysmon::SysStats,
};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::time::{Instant, SystemTime};

const OPTION_HELP: [&str; OPTION_COUNT] = [
    "", // wiersz 0: opis budowany dynamicznie (wykryte enkodery)
    "Jakość HEVC: NVENC -cq, QSV ICQ, VAAPI QP; mniej = lepsza jakość. ←/→ ±1, PgUp/PgDn ±5.",
    "Jakość AV1: NVENC -cq, QSV ICQ, VAAPI QP×5; mniej = lepsza jakość. ←/→ ±1, PgUp/PgDn ±5.",
    "Tylko Intel QSV: ← szybciej, → wolniej i nieco lepiej. veryslow bywa kilka razy wolniejszy od slow.",
    "", // wiersz 4: kontener — opis zależny od wyboru (ostrzeżenie dla MP4)
    "Pierwsza polska ścieżka przekodowana do E-AC-3 2.0 192 kb/s.",
    "Bez tego: tylko polska ścieżka audio i brak napisów.",
    "NVIDIA: bilateral_cuda, Intel: vpp_qsv/denoise_vaapi, zapasowo hqdn3d (CPU); tylko SDR 8-bit.",
    "Bez cropdetect: pełny kadr, najdłuższy etap analizy jest pomijany.",
    "Bez ekstrakcji i wstrzykiwania Dolby Vision — wynik zostaje zwykłym HDR10.",
    "Bez ekstrakcji i wstrzykiwania HDR10+ — zostaje statyczny HDR10.",
    "Kopia do <źródło>/encoded, do 3 prób z weryfikacją SHA-512.",
    "Wyłącza komputer 10 s po zakończeniu (dowolny klawisz anuluje).",
    "Enter — edytuj, Enter — zatwierdź, Esc — anuluj.",
];

// --------------------------------------------------------------- pomocnicze ---

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn tail(s: &str, n: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= n {
        s.to_string()
    } else {
        let keep = n.saturating_sub(1);
        format!("…{}", c[c.len() - keep..].iter().collect::<String>())
    }
}

fn pad(s: &str, n: usize) -> String {
    let l = s.chars().count();
    if l >= n {
        s.chars().take(n).collect()
    } else {
        format!("{s}{}", " ".repeat(n - l))
    }
}

fn pad_left(s: &str, n: usize) -> String {
    let l = s.chars().count();
    if l >= n {
        s.chars().take(n).collect()
    } else {
        format!("{}{s}", " ".repeat(n - l))
    }
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b}")
    } else if v >= 100.0 {
        format!("{:.0}{}", v, U[i])
    } else {
        format!("{:.1}{}", v, U[i])
    }
}

fn fmt_time(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t).format("%d.%m %H:%M").to_string()
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn dialog_block(title: &str, color: Rgb) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(fg(color))
        .title(Line::from(Span::styled(format!(" {title} "), bold(TITLE))))
        .title_alignment(Alignment::Center)
        .style(Style::default().bg(col(DLG_BG)))
}

fn panel_block(title: &str, color: Rgb, active: bool) -> Block<'static> {
    let tstyle = if active {
        Style::default().bg(col(color)).fg(col(TITLE)).add_modifier(Modifier::BOLD)
    } else {
        fg(DIM)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(fg(color))
        .title(Line::from(Span::styled(format!(" {title} "), tstyle)))
        .title_alignment(Alignment::Center)
}

fn header_line(a: &str, b: &str, c: &str, name_w: usize) -> Line<'static> {
    let h = bold(G_MID);
    let sep = fg(DIM);
    Line::from(vec![
        Span::styled(pad(a, name_w), h),
        Span::styled("│", sep),
        Span::styled(pad_left(b, 7), h),
        Span::styled("│", sep),
        Span::styled(pad(c, 11), h),
    ])
}

fn cursor_style(active: bool) -> Style {
    if active {
        Style::default().bg(col(BOX_ALL)).fg(col(TITLE)).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(col(CURSOR_INACTIVE))
    }
}

// ------------------------------------------------------------------ główny ---

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    render_topbar(f, rows[0], &app.opts, app.queue.items.len());
    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
    let left_active = app.focus == Focus::Left;

    // Położenie list (ramka + nagłówek kolumn + stopka) — dla obsługi myszy.
    let list_rect = |r: Rect| Rect {
        x: r.x + 1,
        y: r.y + 2,
        width: r.width.saturating_sub(2),
        height: r.height.saturating_sub(4),
    };
    app.ui.left_list = list_rect(cols[0]);
    app.ui.right_list = list_rect(cols[1]);
    app.ui.fkeys = rows[3];

    // Długa nazwa zaznaczonej pozycji w aktywnym panelu przewija się sama.
    let panel_w = if left_active { cols[0].width } else { cols[1].width };
    let name_w = (panel_w as usize).saturating_sub(2).saturating_sub(20).max(4);
    let sel_label = if left_active {
        app.browser.selected().map(entry_label)
    } else {
        app.queue
            .state
            .selected()
            .and_then(|i| app.queue.items.get(i))
            .map(|q| q.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
    };
    let shift = marquee_shift(app, left_active, sel_label, name_w);
    let (lshift, rshift) = if left_active { (shift, 0) } else { (0, shift) };

    render_left(f, cols[0], &mut app.browser, left_active, lshift);
    render_right(f, cols[1], &mut app.queue, &app.snapshot, !left_active, &app.opts, rshift);
    render_status(f, rows[2], app);
    render_fkeys(f, rows[3]);

    match app.mode {
        Mode::Normal => {}
        Mode::Options => render_options(f, area, app),
        Mode::Help => render_help(f, area),
        Mode::Running | Mode::Finished => render_run(f, area, app),
        Mode::ConfirmAbort => {
            render_run(f, area, app);
            render_confirm(f, area, "Przerwać kodowanie?");
        }
        Mode::ConfirmQuit => {
            render_run(f, area, app);
            render_confirm(f, area, "Kodowanie trwa. Przerwać i wyjść?");
        }
        Mode::ConfirmDelete => render_delete(f, area, &app.pending_delete, &app.pending_delete_info),
        Mode::Rename => {
            if let Some(r) = &app.rename {
                render_rename(f, area, r);
            }
        }
        Mode::Message => {
            if let Some((title, lines)) = &app.message {
                render_message(f, area, title, lines);
            }
        }
        Mode::MediaInfo => {
            if let Some(v) = app.mediainfo.as_mut() {
                render_mediainfo(f, area, v);
            }
        }
        Mode::Find => {
            if let Some(v) = app.find.as_mut() {
                render_find(f, area, v);
            }
        }
        Mode::RenameMany => {
            if let Some(r) = app.rename_many.as_mut() {
                render_rename_many(f, area, r);
            }
        }
    }
}

/// Pole tekstowe: przewijane tak, żeby kursor był widoczny; kursor tylko w aktywnym polu.
fn input_spans(input: &crate::app::TextInput, w: usize, focused: bool) -> Vec<Span<'static>> {
    let chars = &input.chars;
    let start = input.cursor.saturating_sub(w.saturating_sub(1));
    let mut spans = vec![];
    for i in start..start + w {
        let c = chars.get(i).copied().unwrap_or(' ');
        let style = if focused && i == input.cursor {
            Style::default().bg(col(TEXT)).fg(col(DLG_BG))
        } else {
            Style::default().bg(col(CURSOR_INACTIVE)).fg(col(TEXT))
        };
        spans.push(Span::styled(c.to_string(), style));
    }
    spans
}

/// Etykieta pola formularza: podświetlona, gdy pole jest aktywne.
fn field_label(text: &str, w: usize, focused: bool) -> Span<'static> {
    let style = if focused { bold(TITLE).bg(col(BOX_ENC)) } else { fg(TEXT) };
    Span::styled(pad(text, w), style)
}

fn checkbox(text: &str, on: bool, focused: bool) -> Line<'static> {
    let mark = if on { "[x] " } else { "[ ] " };
    let style = if focused { bold(TITLE).bg(col(BOX_ENC)) } else { fg(TEXT) };
    Line::from(vec![Span::raw(" "), Span::styled(format!("{mark}{text}"), style)])
}

/// Kolory nazw różniących się tylko wielkością liter (wariant 1, 2, …).
const CASE_COLORS: [Rgb; 4] = [(0x6e, 0xc5, 0xd6), (0xe0, 0xa0, 0x5a), (0xc0, 0x8f, 0xe0), (0x8f, 0xd4, 0x8b)];

/// Ctrl+F: formularz u góry, pod linią przewijane wyniki z podkreślonym dopasowaniem.
fn render_find(f: &mut Frame, area: Rect, v: &mut crate::find::FindView) {
    use crate::find::{F_AUTO, F_INPUT, F_RECURSIVE, F_RESULTS, F_SHOW};
    let w = area.width.saturating_sub(6).clamp(50, 150);
    let h = area.height.saturating_sub(2).max(14);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = dialog_block("Znajdź plik", BOX_ENC);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let iw = inner.width as usize;

    let rows = Layout::vertical([
        Constraint::Length(5), // formularz
        Constraint::Length(1), // stan
        Constraint::Length(1), // linia
        Constraint::Min(1),    // wyniki
        Constraint::Length(1), // podpowiedź
    ])
    .split(inner);
    v.form_area = rows[0];

    let label_w = 17;
    let mut input_line = vec![Span::raw(" "), field_label("Nazwa zawiera:", label_w, v.field == F_INPUT)];
    input_line.extend(input_spans(&v.input, iw.saturating_sub(label_w + 3), v.field == F_INPUT));
    let form = vec![
        Line::from(vec![
            Span::styled(" Szukaj w:        ", fg(DIM)),
            Span::styled(tail(&v.root.display().to_string(), iw.saturating_sub(19)), fg(TEXT)),
        ]),
        Line::from(input_line),
        checkbox("Szukaj także w podkatalogach", v.recursive, v.field == F_RECURSIVE),
        checkbox("Pokaż wyniki wyszukiwania w oknie", v.show, v.field == F_SHOW),
        checkbox("Dodaj znalezione pliki wideo do kolejki automatycznie", v.auto_add, v.field == F_AUTO),
    ];
    f.render_widget(Paragraph::new(form), rows[0]);

    // Stan wyszukiwania.
    let videos = v.hits.iter().filter(|h| h.is_video()).count();
    let state = if v.searching {
        let dots = ".".repeat((v.started.map(|t| t.elapsed().as_millis() / 400).unwrap_or(0) % 4) as usize);
        format!(" Szukam{dots}  przeszukano: {}  znaleziono: {}", v.scanned, v.hits.len())
    } else if v.query.is_empty() {
        " Wpisz fragment nazwy i naciśnij Enter".to_string()
    } else {
        let mut s = format!(
            " „{}”: znaleziono {} (wideo: {videos}), przeszukano {}",
            v.query,
            v.hits.len(),
            v.scanned
        );
        if v.truncated {
            s.push_str(" — pokazano pierwsze 5000");
        }
        s
    };
    let mut st = vec![Span::styled(trunc(&state, iw), fg(if v.searching { G_MID } else { TEXT }))];
    if !v.info.is_empty() {
        st.push(Span::styled(format!(" · {}", trunc(&v.info, iw.saturating_sub(state.chars().count() + 3))), fg(G_LOW)));
    }
    f.render_widget(Paragraph::new(Line::from(st)), rows[1]);
    let title = format!(" Wyniki · zaznaczone: {} ", v.marked.len());
    let rule = format!("──{title}{}", "─".repeat(iw.saturating_sub(title.chars().count() + 2)));
    f.render_widget(Paragraph::new(Line::from(Span::styled(rule, fg(DIM)))), rows[2]);

    v.list_area = if v.show { rows[3] } else { Rect::default() };
    v.page = rows[3].height as usize;
    if v.show {
        let size_w = 8;
        let name_w = iw.saturating_sub(size_w + 4);
        let items: Vec<ListItem> = v
            .hits
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let marked = v.marked.contains(&i);
                let base = if marked {
                    bold(G_MID)
                } else if h.is_dir {
                    bold(TITLE)
                } else if h.variant > 0 {
                    fg(CASE_COLORS[(h.variant - 1) % CASE_COLORS.len()])
                } else if h.is_video() {
                    fg(TEXT)
                } else {
                    fg(DIM)
                };
                let mark = if marked { "▪" } else { " " };
                let dir = if h.rel_dir.is_empty() { String::new() } else { format!("{}{}", h.rel_dir, std::path::MAIN_SEPARATOR) };
                let prefix = if h.is_dir { "/" } else { "" };
                // Nazwa z podkreślonym dopasowaniem; katalog nadrzędny szarym, skrócony od lewej.
                let name: Vec<char> = h.name.chars().collect();
                let (a, b) = (h.mstart.min(name.len()), (h.mstart + h.mlen).min(name.len()));
                let name_len = prefix.len() + name.len();
                let dir_room = name_w.saturating_sub(name_len + 1);
                let dir_txt = if dir.chars().count() > dir_room { tail(&dir, dir_room) } else { dir };
                let mut spans = vec![
                    Span::styled(mark.to_string(), bold(G_MID)),
                    Span::styled(dir_txt.clone(), fg(DIM)),
                    Span::styled(format!("{prefix}{}", name[..a].iter().collect::<String>()), base),
                    Span::styled(name[a..b].iter().collect::<String>(), base.add_modifier(Modifier::UNDERLINED | Modifier::BOLD)),
                    Span::styled(name[b..].iter().collect::<String>(), base),
                ];
                let used = 1 + dir_txt.chars().count() + name_len;
                spans.push(Span::raw(" ".repeat(name_w.saturating_sub(used) + 1)));
                let size = if h.is_dir { "<KAT>".to_string() } else { human(h.size) };
                spans.push(Span::styled(pad_left(&size, size_w), fg(DIM)));
                ListItem::new(Line::from(spans))
            })
            .collect();
        let hl = cursor_style(v.field == F_RESULTS);
        f.render_stateful_widget(List::new(items).highlight_style(hl), rows[3], &mut v.state);
    } else {
        let msg = " Wyniki ukryte (opcja „Pokaż wyniki wyszukiwania w oknie”).";
        f.render_widget(Paragraph::new(Span::styled(msg, fg(DIM))), rows[3]);
    }
    let hint = if v.field == F_RESULTS {
        " Spacja/klik — zaznacz · + - * · Enter/2×klik — do kolejki (katalog: przejdź) · F5 — dodaj zaznaczone · Tab · Esc"
    } else {
        " Enter — szukaj · Tab/↑↓ — pole · Spacja — przełącz · Esc — zamknij (Ctrl+F wraca do wyników)"
    };
    f.render_widget(Paragraph::new(Span::styled(trunc(hint, iw), fg(G_MID))), rows[4]);
}

/// F4 dla wielu plików: pola szukaj / zamień / rozszerzenie i podgląd zmian.
fn render_rename_many(f: &mut Frame, area: Rect, r: &mut crate::rename_many::RenameMany) {
    use crate::rename_many::{R_EXT, R_FIND, R_REPLACE, R_WITH_EXT};
    let w = area.width.saturating_sub(6).clamp(50, 130);
    let h = area.height.saturating_sub(4).max(14);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = dialog_block("Zmień nazwy wielu plików", BOX_QUEUE);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let iw = inner.width as usize;
    let rows = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let label_w = 15;
    let in_w = iw.saturating_sub(label_w + 3).min(40);
    let field = |label: &str, inp: &crate::app::TextInput, idx: usize, help: &str| {
        let mut l = vec![Span::raw(" "), field_label(label, label_w, r.field == idx)];
        l.extend(input_spans(inp, in_w, r.field == idx));
        l.push(Span::styled(format!("  {help}"), fg(DIM)));
        Line::from(l)
    };
    let scope = if r.marked.is_empty() {
        format!("pliki w {} ({} w zakresie)", tail(&r.dir.display().to_string(), iw.saturating_sub(40)), r.candidates)
    } else {
        format!("zaznaczone pliki ({} w zakresie)", r.candidates)
    };
    let form = vec![
        Line::from(vec![Span::styled(" Zakres: ", fg(DIM)), Span::styled(scope, fg(TEXT))]),
        field("Szukaj:", &r.find, R_FIND, "kilka wariantów: {|}"),
        field("Zamień na:", &r.replace, R_REPLACE, "puste = usuń"),
        field("Rozszerzenie:", &r.ext, R_EXT, "np. mkv; puste = wszystkie"),
        checkbox("Zamieniaj także w rozszerzeniu (np. .MP4 → .mp4)", r.with_ext, r.field == R_WITH_EXT),
        Line::default(),
    ];
    f.render_widget(Paragraph::new(form), rows[0]);

    let conflicts = r.conflicts();
    let title = format!(" Podgląd: zmian {}{} ", r.plans.len(), if conflicts > 0 { format!(", konfliktów {conflicts} (zostaną pominięte)") } else { String::new() });
    let rule = format!("──{title}{}", "─".repeat(iw.saturating_sub(title.chars().count() + 2)));
    f.render_widget(Paragraph::new(Span::styled(rule, fg(if conflicts > 0 { G_HIGH } else { DIM }))), rows[1]);

    r.page = rows[2].height as usize;
    let half = iw.saturating_sub(5) / 2;
    let lines: Vec<Line> = r
        .plans
        .iter()
        .skip(r.scroll)
        .take(rows[2].height as usize)
        .map(|p| {
            let (style, note) = match &p.problem {
                Some(why) => (fg(G_HIGH), format!("  ({why})")),
                None => (fg(TEXT), String::new()),
            };
            Line::from(vec![
                Span::styled(format!(" {}", pad(&p.old, half)), fg(DIM)),
                Span::styled(" → ", fg(G_MID)),
                Span::styled(trunc(&format!("{}{note}", p.new), half), style),
            ])
        })
        .collect();
    let body = if r.plans.is_empty() {
        vec![Line::from(Span::styled(
            if r.find.chars.is_empty() { " Wpisz w „Szukaj” fragment nazwy do zamiany." } else { " Żadna nazwa się nie zmieni." },
            fg(DIM),
        ))]
    } else {
        lines
    };
    f.render_widget(Paragraph::new(body), rows[2]);
    let hint = " Enter — zmień nazwy · Tab/↑↓ — pole · Spacja — przełącz · PgUp/PgDn — podgląd · Esc — anuluj";
    f.render_widget(Paragraph::new(Span::styled(trunc(hint, iw), fg(G_MID))), rows[3]);
}

/// Przesunięcie (w znakach) przewijanej nazwy: postój 1 s, przewijanie ~8 znaków/s, postój 1 s, od nowa.
fn marquee_shift(app: &mut App, left: bool, label: Option<String>, width: usize) -> usize {
    let Some(label) = label else {
        app.marquee.key.clear();
        return 0;
    };
    let overflow = label.chars().count().saturating_sub(width);
    if overflow == 0 {
        app.marquee.key.clear();
        return 0;
    }
    let key = format!("{}{label}", if left { 'L' } else { 'R' });
    if app.marquee.key != key {
        app.marquee.key = key;
        app.marquee.since = std::time::Instant::now();
    }
    const PAUSE: f64 = 1.0;
    const STEP: f64 = 0.12;
    let cycle = PAUSE * 2.0 + overflow as f64 * STEP;
    let t = app.marquee.since.elapsed().as_secs_f64() % cycle;
    if t < PAUSE {
        0
    } else {
        (((t - PAUSE) / STEP) as usize).min(overflow)
    }
}

fn summary(o: &Opts) -> String {
    let mut parts: Vec<String> = vec![o.codec_label()];
    parts.push(format!("CQ {}", o.cq()));
    parts.push(String::from(if o.pl_eac3 { "audio PL E-AC-3 192k" } else { "audio PL (kopia)" }));
    if o.container != ContainerChoice::Auto {
        parts.push(format!("→ {}", o.container.label()));
    }
    if o.extra {
        parts.push("+EN +napisy".into());
    }
    if o.denoise {
        parts.push("odszumianie".into());
    }
    if o.no_crop {
        parts.push("bez crop".into());
    }
    if o.no_dv {
        parts.push("bez DV".into());
    }
    if o.no_hdr10plus {
        parts.push("bez HDR10+".into());
    }
    if o.copy {
        parts.push("kopia → encoded".into());
    }
    if o.shutdown {
        parts.push("wyłącz po zakończeniu".into());
    }
    parts.join(" · ")
}

fn render_topbar(f: &mut Frame, area: Rect, o: &Opts, queued: usize) {
    let mut spans = vec![
        Span::styled(
            format!(" VideoPipeline {} ", env!("CARGO_PKG_VERSION")),
            Style::default().bg(col(BOX_ALL)).fg(col(TITLE)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", summary(o)), fg(DIM)),
    ];
    let right = format!("Kolejka: {queued} ");
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.chars().count());
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, fg(G_MID)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Nazwa pozycji lewego panelu: katalogi z "/", dyski z etykietą (Windows: "D:  Dane",
/// Linux: "/run/media/damian/USB  Pendrive").
fn entry_label(e: &crate::browser::Entry) -> String {
    if e.is_up {
        "/..".to_string()
    } else if e.sep {
        String::new()
    } else if let Some(d) = &e.drive {
        match &d.label {
            Some(l) => format!("{}  {l}", e.name),
            None => e.name.clone(),
        }
    } else if e.is_dir {
        format!("/{}", e.name)
    } else {
        e.name.clone()
    }
}

fn render_left(f: &mut Frame, area: Rect, b: &mut Browser, active: bool, shift: usize) {
    let title = tail(&b.cwd.to_string_lossy(), (area.width as usize).saturating_sub(6));
    let block = panel_block(&title, BOX_ALL, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 3 || inner.width < 24 {
        return;
    }
    let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let name_w = (inner.width as usize).saturating_sub(20).max(4);

    f.render_widget(Paragraph::new(header_line("Nazwa", "Rozmiar", "Zmodyfikowano", name_w)), parts[0]);
    // Dyski przypięte u góry (zawsze widoczne), pod linią przewijana zawartość katalogu.
    let list_h = parts[1].height as usize;
    let pinned_h = b.pinned.min(list_h / 2);
    b.pinned_shown = pinned_h;
    let areas = Layout::vertical([Constraint::Length(pinned_h as u16), Constraint::Min(1)]).split(parts[1]);
    b.page = areas[1].height as usize;

    let sel = b.state.selected();
    let item = |i: usize, e: &crate::browser::Entry| -> ListItem<'static> {
        if e.sep {
            return ListItem::new(Line::from(Span::styled("─".repeat(name_w + 20), fg(DIM))));
        }
        let marked = b.marked.contains(&e.path);
        let label = entry_label(e);
        let style = if e.drive.is_some() {
            bold(G_LOW)
        } else if e.is_dir {
            bold(TITLE)
        } else {
            fg(TEXT)
        };
        let style = if marked { bold(G_MID) } else { style };
        let label = if Some(i) == sel && shift > 0 { label.chars().skip(shift).collect() } else { label };
        let (size, date) = if let Some(d) = &e.drive {
            (
                d.total.map(human).unwrap_or_else(|| "DYSK".into()),
                d.free.map(|f| format!("{} wolne", human(f))).unwrap_or_default(),
            )
        } else if e.is_up {
            ("UP--DIR".to_string(), String::new())
        } else if e.is_dir {
            ("SUB-DIR".to_string(), e.mtime.map(fmt_time).unwrap_or_default())
        } else {
            (human(e.size), e.mtime.map(fmt_time).unwrap_or_default())
        };
        ListItem::new(Line::from(vec![
            Span::styled(pad(&label, name_w), style),
            Span::styled("│", fg(DIM)),
            Span::styled(pad_left(&size, 7), fg(TEXT)),
            Span::styled("│", fg(DIM)),
            Span::styled(pad(&date, 11), fg(DIM)),
        ]))
    };

    if pinned_h > 0 {
        let top: Vec<ListItem> = b.entries[..pinned_h].iter().enumerate().map(|(i, e)| item(i, e)).collect();
        let mut st = ListState::default().with_selected(sel.filter(|s| *s < pinned_h));
        f.render_stateful_widget(List::new(top).highlight_style(cursor_style(active)), areas[0], &mut st);
    }
    let rest: Vec<ListItem> = b.entries[b.pinned.min(b.entries.len())..]
        .iter()
        .enumerate()
        .map(|(i, e)| item(i + b.pinned, e))
        .collect();
    // Kursor na dyskach: lista plików zachowuje przewinięcie, tylko bez podświetlenia.
    let in_rest = sel.and_then(|s| s.checked_sub(b.pinned));
    let hl = if in_rest.is_some() { cursor_style(active) } else { Style::default() };
    if in_rest.is_some() {
        b.rest.select(in_rest);
    }
    f.render_stateful_widget(List::new(rest).highlight_style(hl), areas[1], &mut b.rest);

    let foot = if let Some(err) = &b.error {
        Span::styled(trunc(err, inner.width as usize), fg(G_HIGH))
    } else if !b.marked.is_empty() {
        Span::styled(format!("Zaznaczone: {}", b.marked.len()), bold(G_MID))
    } else if let Some(e) = b.selected() {
        Span::styled(trunc(&e.name, inner.width as usize), fg(DIM))
    } else {
        Span::raw("")
    };
    f.render_widget(Paragraph::new(Line::from(foot)), parts[2]);
}

fn render_right(f: &mut Frame, area: Rect, q: &mut Queue, snap: &Snapshot, active: bool, o: &Opts, shift: usize) {
    let title = format!("Kolejka kodowania ({})", q.items.len());
    let block = panel_block(&title, BOX_ENC, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 3 || inner.width < 24 {
        return;
    }
    let parts = Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let name_w = (inner.width as usize).saturating_sub(20).max(4);

    f.render_widget(Paragraph::new(header_line("Nazwa", "Rozmiar", "Status", name_w)), parts[0]);
    q.page = parts[1].height as usize;

    if q.items.is_empty() {
        let hint = Paragraph::new(vec![
            Line::default(),
            Line::from(Span::styled("Kolejka jest pusta", fg(DIM))),
            Line::from(Span::styled("F5 — dodaj zaznaczone pliki z lewego panelu", fg(DIM))),
        ])
        .alignment(Alignment::Center);
        f.render_widget(hint, parts[1]);
    } else {
        let sel = q.state.selected();
        let items: Vec<ListItem> = q
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let full = it.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let name: String = if Some(i) == sel && shift > 0 { full.chars().skip(shift).collect() } else { full.clone() };
                let marked = q.marked.contains(&it.path);
                let st = snap.items.get(&full).map(|i| i.state);
                let (label, color) = match st {
                    Some(s) => (s.label(), state_color(s)),
                    None => ("", DIM),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(pad(&name, name_w), if marked { bold(G_MID) } else { fg(TEXT) }),
                    Span::styled("│", fg(DIM)),
                    Span::styled(pad_left(&human(it.size), 7), fg(TEXT)),
                    Span::styled("│", fg(DIM)),
                    Span::styled(pad(label, 11), fg(color)),
                ]))
            })
            .collect();
        let list = List::new(items).highlight_style(cursor_style(active));
        f.render_stateful_widget(list, parts[1], &mut q.state);
    }

    let foot = format!("Razem: {} · {} · {}", q.items.len(), human(q.total_size()), o.codec_label());
    f.render_widget(Paragraph::new(Line::from(Span::styled(trunc(&foot, inner.width as usize), fg(DIM)))), parts[2]);
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let line = if app.status_msg.is_empty() {
        Span::styled(
            " Tab — panel · Shift+↑↓ / Spacja — zaznacz · + - * — wszystko/nic/odwróć · Enter — wejdź/dodaj",
            fg(DIM),
        )
    } else {
        Span::styled(format!(" {}", app.status_msg), bold(G_MID))
    };
    f.render_widget(Paragraph::new(Line::from(line)), area);
}

fn render_fkeys(f: &mut Frame, area: Rect) {
    let labels = ["Pomoc", "Info", "Odtwórz", "Nazwa", "Dodaj", "Kasuj", "Opcje", "Z kolejki", "Koduj", "Wyjście"];
    let total = area.width as usize;
    let cell = total / 10;
    let mut spans: Vec<Span<'static>> = vec![];
    for (i, l) in labels.iter().enumerate() {
        let num = format!("{}", i + 1);
        let width = if i == 9 { total.saturating_sub(cell * 9) } else { cell };
        let label = pad(l, width.saturating_sub(num.len()));
        let style = if l.is_empty() {
            Style::default().bg(col(CURSOR_INACTIVE))
        } else {
            Style::default().bg(col(BOX_ALL)).fg(col(TITLE))
        };
        spans.push(Span::styled(num, fg(TEXT)));
        spans.push(Span::styled(label, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// --------------------------------------------------------------- okienka ---

fn render_options(f: &mut Frame, area: Rect, app: &App) {
    let rect = centered(area, 78, 22);
    f.render_widget(Clear, rect);
    let block = dialog_block("Opcje videopipeline", BOX_QUEUE);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let w = inner.width as usize;
    let o = &app.opts;
    let chk = |b: bool| if b { "[x]" } else { "[ ]" };

    let out_val = match &app.opt_edit {
        Some(buf) => format!("{buf}_"),
        None => o.output_dir.to_string_lossy().to_string(),
    };
    let rows: Vec<(String, String)> = vec![
        (
            "Kodek wideo".into(),
            if app.hw.has(o.engine, o.codec) {
                format!("< {} >", o.codec_label())
            } else {
                format!("< {} — niedostępny >", o.codec_label())
            },
        ),
        ("Jakość HEVC — CQ   (--hevc-cq)".into(), format!("< {} >", o.hevc_cq)),
        ("Jakość AV1 — CQ   (--av1-cq)".into(), format!("< {} >", o.av1_cq)),
        (
            "Preset Intel QSV   (--qsv-preset)".into(),
            if o.engine == Engine::Qsv {
                format!("< {} >", o.qsv_preset)
            } else {
                format!("({}, tylko QSV) < {} >", o.engine.label(), o.qsv_preset)
            },
        ),
        ("Kontener wyjściowy   (--container)".into(), format!("< {} >", o.container.label())),
        (format!("{} Audio PL → E-AC-3 stereo 192k   (-pleac3)", chk(o.pl_eac3)), String::new()),
        (format!("{} Dodatkowo audio EN + napisy EN i PL   (-da)", chk(o.extra)), String::new()),
        (format!("{} Odszumianie SDR 8-bit   (-d)", chk(o.denoise)), String::new()),
        (format!("{} Pomiń crop — bez cropdetect   (-pc)", chk(o.no_crop)), String::new()),
        (format!("{} Pomiń analizę Dolby Vision   (-pdv)", chk(o.no_dv)), String::new()),
        (format!("{} Pomiń analizę HDR10+   (-ph)", chk(o.no_hdr10plus)), String::new()),
        (format!("{} Kopiuj do source/encoded + SHA-512   (-c)", chk(o.copy)), String::new()),
        (format!("{} Wyłącz komputer po zakończeniu   (-s)", chk(o.shutdown)), String::new()),
        ("Katalog wyjściowy".into(), tail(&out_val, w.saturating_sub(24))),
    ];

    let mut lines: Vec<Line<'static>> = vec![Line::default()];
    for (i, (left, right)) in rows.iter().enumerate() {
        let gap = w.saturating_sub(2 + left.chars().count() + right.chars().count());
        let text = pad(&format!(" {left}{}{right} ", " ".repeat(gap)), w);
        let style = if i == app.opt_cursor { cursor_style(true) } else { fg(TEXT) };
        lines.push(Line::from(Span::styled(text, style)));
    }
    lines.push(Line::default());
    let help = if app.opt_cursor == 0 {
        format!("Wykryte: {}. HEVC: DV/HDR10+; AV1: 10-bit, bez DV/HDR10+.", app.hw.summary())
    } else if app.opt_cursor == crate::app::OPT_CONTAINER {
        match o.container {
            ContainerChoice::Auto => "Jak źródło: .mp4 → MP4, reszta → MKV. Pełne DV i HDR10+ tylko w MKV.".into(),
            ContainerChoice::Mkv => "MKV: pełne Dolby Vision i HDR10+, wszystkie ścieżki audio i napisy.".into(),
            ContainerChoice::Mp4 => {
                "UWAGA: w MP4 Dolby Vision/HDR10+ nie będą pełne — do nich wybierz MKV.".into()
            }
        }
    } else {
        OPTION_HELP[app.opt_cursor.min(OPTION_COUNT - 1)].to_string()
    };
    lines.push(Line::from(Span::styled(format!(" {}", trunc(&help, w.saturating_sub(2))), fg(DIM))));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        " ↑↓ wybór · Spacja/Enter/←→ zmień · CQ: ←→ ±1, PgUp/PgDn ±5 · Esc/F7 zamknij",
        fg(G_MID),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_help(f: &mut Frame, area: Rect) {
    let rect = centered(area, 68, 29);
    f.render_widget(Clear, rect);
    let block = dialog_block("Pomoc", BOX_ALL);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let k = |key: &str, desc: &str| {
        Line::from(vec![
            Span::styled(format!(" {:<16}", key), bold(G_MID)),
            Span::styled(desc.to_string(), fg(TEXT)),
        ])
    };
    let lines = vec![
        Line::default(),
        k("Tab", "przełącz panel (pliki ↔ kolejka)"),
        k("↑ ↓ PgUp PgDn", "ruch kursora"),
        k("Shift+↑ ↓ PgUp PgDn", "zaznaczaj wiele pozycji (jak w MC)"),
        k("Spacja / Ins", "zaznacz / odznacz bieżącą pozycję"),
        k("+  -  *", "zaznacz wszystko / wyczyść / odwróć"),
        k("Enter", "katalog: wejdź · plik: dodaj do kolejki"),
        k("Backspace", "katalog wyżej"),
        k("F2", "informacje o pliku (MediaInfo); prawy przycisk myszy też"),
        k("F3", "odtwórz plik w mpv (wymaga środowiska graficznego)"),
        k("F4", "zmień nazwę; kilka zaznaczonych / Tab — wiele naraz"),
        k("F5", "dodaj zaznaczone (lub bieżący) do kolejki"),
        k("Ctrl+F", "znajdź plik po nazwie (bieżący katalog / podkatalogi)"),
        k("F6", "usuń zaznaczone pliki/katalogi z dysku (lewy panel)"),
        k("F7", "opcje: kodek i karta (NVENC/QSV/VAAPI), audio, analizy…"),
        k("F8 / Del", "usuń zaznaczone (lub bieżący) z kolejki"),
        k("F9", "uruchom kodowanie"),
        k("Ctrl+L", "przerysuj ekran"),
        k("F10 / Ctrl+C", "wyjście"),
        Line::default(),
        Line::from(Span::styled(" Katalog jako cel F5 dodaje wszystkie filmy z niego.", fg(DIM))),
        Line::from(Span::styled(" Dowolny klawisz zamyka pomoc.", fg(DIM))),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// F6: potwierdzenie usunięcia plików z dysku (lista do 5 nazw).
fn render_delete(f: &mut Frame, area: Rect, files: &[std::path::PathBuf], info: &crate::app::DeleteInfo) {
    let shown = files.len().min(5);
    let warn_lines: u16 = if info.dirs > 0 { 3 } else { 0 };
    let h = 7 + warn_lines + shown as u16 + u16::from(files.len() > shown);
    let rect = centered(area, 72, h);
    f.render_widget(Clear, rect);
    let block = dialog_block("Usuń z dysku", BOX_ERR);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let w = inner.width as usize;
    let n_files = files.len() - info.dirs;
    let what = match (n_files, info.dirs) {
        (0, d) => format!("katalogów: {d}"),
        (n, 0) => format!("plików: {n}"),
        (n, d) => format!("plików: {n} i katalogów: {d}"),
    };
    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled(format!("Usunąć z dysku {what}?"), bold(TITLE))),
        Line::from(Span::styled("Tej operacji nie można cofnąć — pliki nie trafiają do kosza.", fg(G_HIGH))),
    ];
    if info.dirs > 0 {
        let more = if info.truncated { "ponad " } else { "" };
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "UWAGA: katalogi zostaną usunięte RAZEM Z CAŁĄ ZAWARTOŚCIĄ",
            bold(G_HIGH),
        )));
        lines.push(Line::from(Span::styled(
            format!("(w środku {more}{} plików, {more}{})", info.inner_files, human(info.inner_bytes)),
            fg(G_HIGH),
        )));
    }
    lines.push(Line::default());
    for p in files.iter().take(shown) {
        let mut n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false) {
            n = format!("/{n}");
        }
        lines.push(Line::from(Span::styled(trunc(&n, w.saturating_sub(2)), fg(TEXT))));
    }
    if files.len() > shown {
        lines.push(Line::from(Span::styled(format!("… i {} więcej", files.len() - shown), fg(DIM))));
    }
    let keys = if info.dirs > 0 {
        "Enter / [T]ak — usuń razem z zawartością · Esc / [N]ie"
    } else {
        "Enter / [T]ak · Esc / [N]ie"
    };
    lines.push(Line::from(Span::styled(keys, fg(G_MID))));
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

/// F4: pole z nową nazwą; kursor jako odwrócony znak.
fn render_rename(f: &mut Frame, area: Rect, r: &crate::app::RenameCtx) {
    let rect = centered(area, 72, 7);
    f.render_widget(Clear, rect);
    let block = dialog_block("Zmień nazwę", BOX_QUEUE);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let w = (inner.width as usize).saturating_sub(2).max(10);

    // Okno przewijane tak, żeby kursor był widoczny.
    let chars = &r.input.chars;
    let start = r.input.cursor.saturating_sub(w.saturating_sub(1));
    let mut spans = vec![Span::raw(" ")];
    for i in start..(start + w).min(chars.len() + 1) {
        let c = chars.get(i).copied().unwrap_or(' ');
        let style = if i == r.input.cursor {
            Style::default().bg(col(TEXT)).fg(col(DLG_BG))
        } else {
            Style::default().bg(col(CURSOR_INACTIVE)).fg(col(TEXT))
        };
        spans.push(Span::styled(c.to_string(), style));
    }
    let old = r.from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let lines = vec![
        Line::from(Span::styled(format!(" Obecnie: {}", trunc(&old, w.saturating_sub(10))), fg(DIM))),
        Line::default(),
        Line::from(spans),
        Line::default(),
        Line::from(Span::styled(" Enter — zapisz · Tab — wiele plików · Esc — anuluj · ←→ Home End", fg(G_MID))),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// F2: raport MediaInfo w stylu widoku „Easy” — sekcje z jedną linią na strumień, przewijane.
fn render_mediainfo(f: &mut Frame, area: Rect, v: &mut crate::app::MediaInfoView) {
    let w = area.width.saturating_sub(4).clamp(40, 110);
    let inner_w = (w as usize).saturating_sub(4);
    let mut lines: Vec<Line<'static>> = vec![];
    match &v.report {
        Err(e) => {
            lines.push(Line::from(Span::styled("Nie udało się odczytać informacji:", bold(G_HIGH))));
            for l in wrap(e, inner_w) {
                lines.push(Line::from(Span::styled(l, fg(TEXT))));
            }
        }
        Ok(sections) => {
            for (n, sec) in sections.iter().enumerate() {
                if n > 0 {
                    lines.push(Line::default());
                }
                let head = format!("─ {} ", sec.title);
                let fill = inner_w.saturating_sub(head.chars().count());
                lines.push(Line::from(vec![
                    Span::styled(head, bold(TITLE)),
                    Span::styled("─".repeat(fill), fg(BOX_QUEUE)),
                ]));
                for l in &sec.lines {
                    for part in wrap(l, inner_w.saturating_sub(1)) {
                        lines.push(Line::from(Span::styled(format!(" {part}"), fg(TEXT))));
                    }
                }
            }
        }
    }
    let body_h = lines.len() as u16;
    let h = (body_h + 4).min(area.height.saturating_sub(2)).max(8);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = dialog_block(&format!("MediaInfo — {}", trunc(&v.file, (w as usize).saturating_sub(18))), BOX_ALL);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let view_h = parts[0].height as usize;
    v.max_scroll = lines.len().saturating_sub(view_h);
    v.scroll = v.scroll.min(v.max_scroll);
    let visible: Vec<Line> = lines.into_iter().skip(v.scroll).take(view_h).collect();
    f.render_widget(Paragraph::new(visible), parts[0]);
    let more = if v.max_scroll > 0 { " · ↑↓ PgUp PgDn — przewijanie" } else { "" };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("Esc / F2 — zamknij{more}"), fg(G_MID)))),
        parts[1],
    );
}

/// Okienko komunikatu (np. błąd uprawnień przy F6): zawinięte linie, Enter/Esc zamyka.
fn render_message(f: &mut Frame, area: Rect, title: &str, text: &[String]) {
    let w = area.width.saturating_sub(4).clamp(40, 100);
    let inner_w = (w as usize).saturating_sub(4);
    let mut lines: Vec<Line<'static>> = vec![Line::default()];
    for (i, l) in text.iter().enumerate() {
        // Pierwsza linia każdego bloku (po pustej) — pogrubiona.
        let head = i == 0 || text.get(i.wrapping_sub(1)).is_some_and(|p| p.is_empty());
        let style = if head { bold(G_HIGH) } else { fg(TEXT) };
        if l.is_empty() {
            lines.push(Line::default());
            continue;
        }
        let indent = l.len() - l.trim_start().len();
        for (n, part) in wrap(l.trim_start(), inner_w.saturating_sub(indent + 1)).into_iter().enumerate() {
            let pad = if n == 0 { indent } else { indent + 2 };
            lines.push(Line::from(Span::styled(format!(" {}{part}", " ".repeat(pad)), style)));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(" Enter / Esc — zamknij", fg(G_MID))));
    let h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let block = dialog_block(title, BOX_ERR);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(lines), inner);
}

/// Zawijanie tekstu po słowach do szerokości `w` (w znakach).
fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(10);
    let mut out = vec![];
    let mut cur = String::new();
    for word in s.split(' ') {
        let wl = word.chars().count();
        let cl = cur.chars().count();
        if cl > 0 && cl + 1 + wl > w {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
        while cur.chars().count() > w {
            let head: String = cur.chars().take(w).collect();
            cur = cur.chars().skip(w).collect();
            out.push(head);
        }
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

fn render_confirm(f: &mut Frame, area: Rect, text: &str) {
    let rect = centered(area, 54, 5);
    f.render_widget(Clear, rect);
    let block = dialog_block("Potwierdzenie", BOX_ERR);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::default(),
        Line::from(Span::styled(text.to_string(), bold(TITLE))),
        Line::from(Span::styled("Enter / [T]ak · Esc / [N]ie", fg(G_MID))),
    ];
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), inner);
}

fn render_run(f: &mut Frame, area: Rect, app: &App) {
    let rect = centered(area, area.width * 9 / 10, area.height * 85 / 100);
    f.render_widget(Clear, rect);

    let status = app.run.as_ref().map(|r| r.last.clone()).unwrap_or(RunStatus::Running);
    let (title, color) = match &status {
        RunStatus::Running => ("Kodowanie", BOX_ENC),
        RunStatus::Done => ("Zakończono", BOX_ALL),
        RunStatus::Failed(_) => ("Błąd", BOX_ERR),
        RunStatus::Cancelled => ("Przerwano", BOX_QUEUE),
    };
    let block = dialog_block(title, color);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 4 || inner.width < 30 {
        return;
    }

    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let body = Rect { x: parts[0].x + 1, width: parts[0].width.saturating_sub(2), ..parts[0] };
    let cw = body.width as usize;

    let mut lines: Vec<Line<'static>> = vec![];
    if let RunStatus::Failed(msg) = &status {
        lines.push(Line::from(Span::styled(trunc(msg, cw), bold(G_HIGH))));
        lines.push(Line::default());
    }
    let used = lines.len();
    let show_sys = matches!(status, RunStatus::Running);
    let eng = app.run.as_ref().map(|r| r.opts.engine).unwrap_or(app.opts.engine);
    lines.extend(run_lines(
        &app.snapshot,
        &app.sys,
        eng.label(),
        show_sys,
        cw,
        (body.height as usize).saturating_sub(used),
    ));
    lines.truncate(body.height as usize);
    f.render_widget(Paragraph::new(lines), body);

    let hint = match &status {
        RunStatus::Running => Span::styled(" Esc / F10 — przerwij kodowanie", fg(DIM)),
        _ => match app.shutdown_at {
            Some(t) => {
                let secs = t.saturating_duration_since(Instant::now()).as_secs() + 1;
                Span::styled(
                    format!(" Wyłączenie komputera za {secs} s — dowolny klawisz anuluje"),
                    bold(G_HIGH),
                )
            }
            None => Span::styled(" Dowolny klawisz — zamknij (gotowe pliki znikną z kolejki)", fg(G_MID)),
        },
    };
    f.render_widget(Paragraph::new(Line::from(hint)), parts[1]);
}

fn run_lines(
    snap: &Snapshot,
    sys: &SysStats,
    engine: &str,
    show_sys: bool,
    cw: usize,
    h: usize,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = vec![];

    let rows: Vec<(&String, &Item, f64, Option<Enc>)> = snap
        .items
        .iter()
        .map(|(n, it)| {
            let enc = if it.state == State::Nvenc { enc_progress(it) } else { None };
            let ov = overall_pct(it, &enc);
            (n, it, ov, enc)
        })
        .collect();

    let total = rows.len().max(1);
    let avg = rows.iter().map(|r| r.2).sum::<f64>() / total as f64;
    let done = rows.iter().filter(|r| r.1.state == State::Done).count();
    let active = rows
        .iter()
        .filter(|r| {
            r.3.is_some()
                || matches!(
                    r.1.state,
                    State::Analysis | State::Hdr | State::Hdr10Plus | State::Dv | State::Audio | State::Mux
                )
        })
        .count();

    // --- ogólny postęp ---------------------------------------------------------
    let mut all = vec![Span::styled("ALL   ", fg(DIM))];
    all.extend(meter(avg, cw.saturating_sub(13)));
    all.push(pct_span(avg));
    out.push(Line::from(all));
    let stage = if snap.title.is_empty() { "start" } else { snap.title.as_str() };
    out.push(Line::from(vec![
        Span::styled("Etap ", fg(DIM)),
        Span::styled(trunc(stage, cw.saturating_sub(40)), fg(TEXT)),
        Span::styled("   Gotowe ", fg(DIM)),
        Span::styled(format!("{done}/{}", rows.len()), fg(G_LOW)),
        Span::styled("   Aktywne ", fg(DIM)),
        Span::styled(active.to_string(), fg(G_MID)),
    ]));
    out.push(Line::default());

    // --- obciążenie: GPU (NVIDIA albo Intel) / CPU / RAM ---------------------------
    if show_sys && h >= 16 {
        out.extend(sys_lines(sys, cw));
        out.push(Line::default());
    }

    // --- analiza (etap 1) -------------------------------------------------------
    if let Some(a) = &snap.analysis {
        out.push(Line::from(Span::styled(trunc(&a.name, cw), bold(TITLE))));
        let mut l = vec![Span::styled("ANALIZA ", fg(DIM))];
        l.extend(meter(a.pct, cw.saturating_sub(8 + 7)));
        l.push(pct_span(a.pct));
        out.push(Line::from(l));
        out.push(Line::from(vec![
            Span::styled("KROK: ", fg(DIM)),
            Span::styled(trunc(&a.step, cw.saturating_sub(6)), fg(TEXT)),
        ]));
        out.push(Line::from(vec![
            Span::styled("INFO: ", fg(DIM)),
            Span::styled(trunc(&a.note, cw.saturating_sub(6)), fg(TEXT)),
        ]));
        out.push(Line::default());
    }

    // --- aktywne kodowania: mierniki + wykres fps (jak CPU w btop) ---------------
    let g_rows = if h >= 28 { 2 } else { 1 };
    for r in rows.iter().filter(|r| r.3.is_some()) {
        let e = r.3.as_ref().unwrap();
        let mw = cw.saturating_sub(6 + 7);
        out.push(Line::from(Span::styled(trunc(r.0, cw), bold(TITLE))));

        let mut l1 = vec![Span::styled("PIPE  ", fg(DIM))];
        l1.extend(meter(r.2, mw));
        l1.push(pct_span(r.2));
        out.push(Line::from(l1));

        let mut l2 = vec![Span::styled(pad(engine, 6), fg(DIM))];
        l2.extend(meter(e.pct, mw));
        l2.push(pct_span(e.pct));
        out.push(Line::from(l2));

        out.extend(graph(&r.1.history, cw, g_rows));

        let speed = e.prog.speed.clone().unwrap_or_else(|| "--".into());
        let fps = e.prog.fps.clone().unwrap_or_else(|| "--".into());
        let snum = speed.trim_end_matches('x').parse::<f64>().unwrap_or(0.0);
        let eta = if snum > 0.0 && e.dur > e.cur { hms((e.dur - e.cur) / snum) } else { "--:--:--".into() };
        out.push(Line::from(vec![
            Span::styled("fps ", fg(DIM)),
            Span::styled(fps, fg(TEXT)),
            Span::styled("  speed ", fg(DIM)),
            Span::styled(speed, fg(TEXT)),
            Span::styled("  ETA ", fg(DIM)),
            Span::styled(eta, fg(G_MID)),
            Span::styled(format!("  {}/{}", hms(e.cur), hms(e.dur)), fg(DIM)),
        ]));
        if !r.1.note.is_empty() && h >= 24 {
            out.push(Line::from(Span::styled(trunc(&r.1.note, cw), fg(DIM))));
        }
        out.push(Line::default());
    }

    // --- pozostałe pliki ---------------------------------------------------------
    let rest: Vec<&(&String, &Item, f64, Option<Enc>)> = rows.iter().filter(|r| r.3.is_none()).collect();
    let available = h.saturating_sub(out.len());
    let reserve_log = (available / 3).min(5);
    let queue_cap = available.saturating_sub(reserve_log + 1);
    let note_w = cw.saturating_sub(2 + 28 + 1 + 12 + 1 + 16 + 7 + 1);
    for (i, r) in rest.iter().enumerate() {
        if i >= queue_cap {
            out.push(Line::from(Span::styled(format!("  … i {} więcej", rest.len() - i), fg(DIM))));
            break;
        }
        let c = state_color(r.1.state);
        let mut l = vec![
            Span::styled("● ", fg(c)),
            Span::styled(pad(r.0, 28), fg(TEXT)),
            Span::raw(" "),
            Span::styled(pad(r.1.state.label(), 12), fg(c)),
            Span::raw(" "),
        ];
        l.extend(meter(r.2, 16));
        l.push(pct_span(r.2));
        l.push(Span::styled(format!(" {}", trunc(&r.1.note, note_w)), fg(DIM)));
        out.push(Line::from(l));
    }

    // --- log ------------------------------------------------------------------------
    let space = h.saturating_sub(out.len());
    if space >= 2 {
        out.push(Line::from(Span::styled(format!("─ log {}", "─".repeat(cw.saturating_sub(6))), fg(BOX_QUEUE))));
        for l in log::tail(space - 1) {
            out.push(Line::from(Span::styled(trunc(&l, cw), fg(DIM))));
        }
    }
    out
}

// ------------------------------------------------------------- obciążenie ---

/// Jedna połowa wiersza: etykieta + miernik + procent (+ opcjonalny opis, np. "3.2/8.0G").
fn half(label: &str, pct: Option<f64>, extra: &str, w: usize) -> Vec<Span<'static>> {
    let ex = if extra.is_empty() { 0 } else { extra.chars().count() + 1 };
    let mw = w.saturating_sub(7 + 7 + ex);
    let mut v = vec![Span::styled(pad(label, 7), fg(DIM))];
    match pct {
        Some(p) => {
            v.extend(meter(p, mw));
            v.push(pct_span(p));
        }
        None => v.push(Span::styled(pad("brak danych", mw + 7), fg(DIM))),
    }
    if ex > 0 {
        v.push(Span::styled(format!(" {extra}"), fg(DIM)));
    }
    v
}

fn pair(mut a: Vec<Span<'static>>, b: Vec<Span<'static>>) -> Line<'static> {
    a.push(Span::styled(" │ ", fg(DIM)));
    a.extend(b);
    Line::from(a)
}

/// Intel: VIDEO = blok VDBox (koduje i dekoduje — sprzętowo jedna jednostka), VPP = VEBox (filtry vpp).
fn intel_lines(s: &SysStats, i: &IntelStats, cw: usize) -> Vec<Line<'static>> {
    let w = cw.saturating_sub(3) / 2;
    let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
    let freq = match (i.freq_mhz, i.freq_max_mhz) {
        (Some(c), Some(m)) => format!(" · {c}/{m} MHz"),
        (Some(c), None) => format!(" · {c} MHz"),
        _ => String::new(),
    };
    let (ram_pct, ram_txt) = match (s.ram_used, s.ram_total) {
        (Some(u), Some(t)) if t > 0 => (Some(100.0 * u as f64 / t as f64), format!("{:.1}/{:.1}G", gib(u), gib(t))),
        _ => (None, String::new()),
    };
    let mem = match i.mem_bytes {
        Some(b) => format!("pamięć GPU (ffmpeg): {:.2}G", gib(b)),
        None => "pamięć GPU: brak danych".into(),
    };
    let header = format!("─ obciążenie · {}{freq}", i.name);
    // Filtry vpp_qsv sterownik często wykonuje na jednostkach render (EU), a nie na VEBox —
    // dlatego jeden wskaźnik: większe z obciążeń RENDER/COMPUTE i VIDEO-ENHANCE.
    let filters = match (i.render, i.enhance) {
        (Some(r), Some(e)) => Some(r.max(e)),
        (r, e) => r.or(e),
    };
    vec![
        Line::from(Span::styled(trunc(&header, cw), fg(BOX_QUEUE))),
        pair(half("VIDEO", i.video, "kod+dek", w), half("FILTRY", filters, "vpp/EU", w)),
        pair(half("CPU", s.cpu, "", w), half("RAM", ram_pct, &ram_txt, w)),
        Line::from(Span::styled(trunc(&mem, cw), fg(DIM))),
    ]
}

fn sys_lines(s: &SysStats, cw: usize) -> Vec<Line<'static>> {
    if let Some(i) = &s.intel {
        return intel_lines(s, i, cw);
    }
    let w = cw.saturating_sub(3) / 2;
    let g = s.gpu.as_ref();
    let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);

    let (vram_pct, vram_txt) = match g.and_then(|g| Some((g.vram_used_mib?, g.vram_total_mib?))) {
        Some((u, t)) if t > 0.0 => (Some(100.0 * u / t), format!("{:.1}/{:.1}G", u / 1024.0, t / 1024.0)),
        _ => (None, String::new()),
    };
    let (ram_pct, ram_txt) = match (s.ram_used, s.ram_total) {
        (Some(u), Some(t)) if t > 0 => (Some(100.0 * u as f64 / t as f64), format!("{:.1}/{:.1}G", gib(u), gib(t))),
        _ => (None, String::new()),
    };
    let name = g
        .map(|g| g.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "nvidia-smi: brak danych".into());

    vec![
        Line::from(Span::styled(format!("─ obciążenie · {}", trunc(&name, cw.saturating_sub(16))), fg(BOX_QUEUE))),
        pair(half("GPU", g.and_then(|g| g.gpu), "", w), half("NVENC", g.and_then(|g| g.enc), "", w)),
        pair(half("NVDEC", g.and_then(|g| g.dec), "", w), half("VRAM", vram_pct, &vram_txt, w)),
        pair(half("CPU", s.cpu, "", w), half("RAM", ram_pct, &ram_txt, w)),
    ]
}
