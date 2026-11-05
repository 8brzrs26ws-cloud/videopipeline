//! Paleta (motyw btop) i elementy graficzne: mierniki z gradientem, wykres braille.
use videopipeline_core::model::State;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub type Rgb = (u8, u8, u8);

pub const G_LOW: Rgb = (0x77, 0xca, 0x9b);
pub const G_MID: Rgb = (0xcb, 0xc0, 0x6c);
pub const G_HIGH: Rgb = (0xdc, 0x4c, 0x4c);
pub const METER_BG: Rgb = (0x40, 0x40, 0x40);
pub const TEXT: Rgb = (0xcc, 0xcc, 0xcc);
pub const TITLE: Rgb = (0xee, 0xee, 0xee);
pub const DIM: Rgb = (0x80, 0x80, 0x80);
pub const BOX_ALL: Rgb = (0x55, 0x6d, 0x59); // ramka CPU w btop
pub const BOX_ENC: Rgb = (0x5c, 0x58, 0x8d); // ramka NET
pub const BOX_QUEUE: Rgb = (0x6c, 0x6c, 0x4b); // ramka MEM
pub const BOX_ERR: Rgb = (0x80, 0x52, 0x52); // ramka PROC
pub const DLG_BG: Rgb = (0x16, 0x16, 0x16);
pub const CURSOR_INACTIVE: Rgb = (0x30, 0x30, 0x30);

pub fn col(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

pub fn fg(c: Rgb) -> Style {
    Style::default().fg(col(c))
}

pub fn bold(c: Rgb) -> Style {
    fg(c).add_modifier(Modifier::BOLD)
}

fn lerp(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let f = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

pub fn gradient(t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        lerp(G_LOW, G_MID, t * 2.0)
    } else {
        lerp(G_MID, G_HIGH, (t - 0.5) * 2.0)
    }
}

pub fn state_color(s: State) -> Rgb {
    match s {
        State::Waiting => DIM,
        State::Analysis => (0x6e, 0xc5, 0xd6),
        State::Nvenc => (0x8f, 0x8b, 0xd4),
        State::NvencDone | State::Done => G_LOW,
        State::Hdr | State::Hdr10Plus | State::Dv | State::Audio => G_MID,
        State::Mux => (0xe0, 0xa0, 0x5a),
        State::Error => G_HIGH,
    }
}

/// Miernik jak w btop: "■", kolor zależy od pozycji komórki na pasku.
pub fn meter(pct: f64, width: usize) -> Vec<Span<'static>> {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let denom = (width.max(2) - 1) as f64;
    (0..width)
        .map(|i| {
            let c = if i < filled { gradient(i as f64 / denom) } else { METER_BG };
            Span::styled("■", fg(c))
        })
        .collect()
}

/// " 45.2%" (7 znaków), kolor wg wartości.
pub fn pct_span(p: f64) -> Span<'static> {
    Span::styled(format!(" {:5.1}%", p.clamp(0.0, 100.0)), fg(gradient(p / 100.0)))
}

const LEFT: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
const RIGHT: [u32; 4] = [0x80, 0x20, 0x10, 0x08];

/// Wykres powierzchniowy z braille (2 próbki na znak, 4 poziomy na wiersz).
pub fn graph(hist: &[f64], width: usize, rows: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let samples = width * 2;
    let max = hist.iter().cloned().fold(0.0_f64, f64::max).max(1.0) * 1.15;
    let start = hist.len().saturating_sub(samples);
    let data = &hist[start..];
    let pad = samples - data.len();
    let top = (rows * 4) as f64;

    let levels: Vec<usize> = (0..samples)
        .map(|i| if i < pad { 0 } else { ((data[i - pad] / max) * top).round() as usize })
        .collect();

    let mut out = Vec::new();
    for r in (0..rows).rev() {
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(width);
        for c in 0..width {
            let (l, rt) = (levels[2 * c], levels[2 * c + 1]);
            let dots = |lv: usize| lv.saturating_sub(r * 4).min(4);
            let mut bits = 0u32;
            for k in 0..dots(l) {
                bits |= LEFT[k];
            }
            for k in 0..dots(rt) {
                bits |= RIGHT[k];
            }
            if bits == 0 {
                spans.push(Span::raw(" "));
            } else {
                let frac = l.max(rt) as f64 / top;
                let ch = char::from_u32(0x2800 + bits).unwrap_or(' ');
                spans.push(Span::styled(ch.to_string(), fg(gradient(frac))));
            }
        }
        out.push(Line::from(spans));
    }
    out
}
