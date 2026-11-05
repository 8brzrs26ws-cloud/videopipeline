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
    sysmon::SysStats,
};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};
use std::time::{Instant, SystemTime};

const OPTION_HELP: [&str; OPTION_COUNT] = [
    "HEVC: obsługa DV/HDR10+ (dovi_tool). AV1: 10-bit, bez DV/HDR10+.",
    "Jakość HEVC (NVENC -cq): mniej = lepsza jakość i większy plik. ←/→ ±1, PgUp/PgDn ±5.",
    "Jakość AV1 (NVENC -cq): mniej = lepsza jakość i większy plik. ←/→ ±1, PgUp/PgDn ±5.",
    "Pierwsza polska ścieżka przekodowana do E-AC-3 2.0 192 kb/s.",
    "Bez tego: tylko polska ścieżka audio i brak napisów.",
    "bilateral_cuda (GPU) albo hqdn3d (CPU); tylko źródła SDR 8-bit.",
    "Bez cropdetect: pełny kadr, najdłuższy etap analizy jest pomijany.",
    "Bez ekstrakcji i wstrzykiwania Dolby Vision — wynik zostaje zwykłym HDR10.",
    "Bez ekstrakcji i wstrzykiwania HDR10+ — zostaje statyczny HDR10.",
    "Kopia do <źródło>/encoded, do 3 prób z weryfikacją SHA-512.",
    "shutdown now po 10 s od końca (dowolny klawisz anuluje).",
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
    render_left(f, cols[0], &mut app.browser, left_active);
    render_right(f, cols[1], &mut app.queue, &app.snapshot, !left_active, &app.opts);
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
    }
}

fn summary(o: &Opts) -> String {
    let mut parts: Vec<String> = vec![String::from(match o.codec {
        Codec::Hevc => "HEVC NVENC",
        Codec::Av1 => "AV1 NVENC",
    })];
    parts.push(format!("CQ {}", o.cq()));
    parts.push(String::from(if o.pl_eac3 { "audio PL E-AC-3 192k" } else { "audio PL (kopia)" }));
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
            " VideoPipeline v11 ",
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

fn render_left(f: &mut Frame, area: Rect, b: &mut Browser, active: bool) {
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
    b.page = parts[1].height as usize;

    let items: Vec<ListItem> = b
        .entries
        .iter()
        .map(|e| {
            let marked = b.marked.contains(&e.path);
            let (label, style) = if e.is_up {
                ("/..".to_string(), bold(TITLE))
            } else if e.is_dir {
                (format!("/{}", e.name), bold(TITLE))
            } else {
                (e.name.clone(), fg(TEXT))
            };
            let style = if marked { bold(G_MID) } else { style };
            let size = if e.is_up {
                "UP--DIR".to_string()
            } else if e.is_dir {
                "SUB-DIR".to_string()
            } else {
                human(e.size)
            };
            let date = e.mtime.map(fmt_time).unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(pad(&label, name_w), style),
                Span::styled("│", fg(DIM)),
                Span::styled(pad_left(&size, 7), fg(TEXT)),
                Span::styled("│", fg(DIM)),
                Span::styled(pad(&date, 11), fg(DIM)),
            ]))
        })
        .collect();
    let list = List::new(items).highlight_style(cursor_style(active));
    f.render_stateful_widget(list, parts[1], &mut b.state);

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

fn render_right(f: &mut Frame, area: Rect, q: &mut Queue, snap: &Snapshot, active: bool, o: &Opts) {
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
        let items: Vec<ListItem> = q
            .items
            .iter()
            .map(|it| {
                let name = it.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let marked = q.marked.contains(&it.path);
                let st = snap.items.get(&name).map(|i| i.state);
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

    let foot = format!(
        "Razem: {} · {} · {}",
        q.items.len(),
        human(q.total_size()),
        match o.codec {
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    );
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
    let labels = ["Pomoc", "", "", "", "Dodaj", "", "Opcje", "Usuń", "Koduj", "Wyjście"];
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
    let rect = centered(area, 78, 20);
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
            format!(
                "< {} >",
                match o.codec {
                    Codec::Hevc => "HEVC NVENC",
                    Codec::Av1 => "AV1 NVENC",
                }
            ),
        ),
        ("Jakość HEVC — CQ   (--hevc-cq)".into(), format!("< {} >", o.hevc_cq)),
        ("Jakość AV1 — CQ   (--av1-cq)".into(), format!("< {} >", o.av1_cq)),
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
    lines.push(Line::from(Span::styled(
        format!(" {}", trunc(OPTION_HELP[app.opt_cursor.min(OPTION_COUNT - 1)], w.saturating_sub(2))),
        fg(DIM),
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        " ↑↓ wybór · Spacja/Enter/←→ zmień · CQ: ←→ ±1, PgUp/PgDn ±5 · Esc/F7 zamknij",
        fg(G_MID),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_help(f: &mut Frame, area: Rect) {
    let rect = centered(area, 68, 22);
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
        k("F5", "dodaj zaznaczone (lub bieżący) do kolejki"),
        k("F7", "opcje: kodek, audio, odszumianie, pomijanie analiz…"),
        k("F8 / Del", "usuń zaznaczone (lub bieżący) z kolejki"),
        k("F9", "uruchom kodowanie"),
        k("F10 / Ctrl+C", "wyjście"),
        Line::default(),
        Line::from(Span::styled(" Katalog jako cel F5 dodaje wszystkie filmy z niego.", fg(DIM))),
        Line::from(Span::styled(" Dowolny klawisz zamyka pomoc.", fg(DIM))),
    ];
    f.render_widget(Paragraph::new(lines), inner);
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
        Line::from(Span::styled("[T]ak / [N]ie", fg(G_MID))),
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
    lines.extend(run_lines(&app.snapshot, &app.sys, show_sys, cw, (body.height as usize).saturating_sub(used)));
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

fn run_lines(snap: &Snapshot, sys: &SysStats, show_sys: bool, cw: usize, h: usize) -> Vec<Line<'static>> {
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

    // --- obciążenie: GPU / NVENC / NVDEC / VRAM / CPU / RAM -----------------------
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

        let mut l2 = vec![Span::styled("NVENC ", fg(DIM))];
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
    let mw = w.saturating_sub(6 + 7 + ex);
    let mut v = vec![Span::styled(pad(label, 6), fg(DIM))];
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

fn sys_lines(s: &SysStats, cw: usize) -> Vec<Line<'static>> {
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
