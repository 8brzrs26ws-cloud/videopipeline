//! Przeciąganie plików na okno (drag & drop). Terminal nie przekazuje programowi zdarzenia „upuść”,
//! tylko wkleja ścieżki jako tekst — z bracketed paste (Event::Paste) albo, np. w konsoli Windows,
//! jako szybką serię znaków. Tu rozpoznajemy w takim tekście ścieżki istniejących plików.
use std::path::PathBuf;

/// Ścieżki istniejących plików/katalogów z wklejonego tekstu. Obsługiwane zapisy terminali:
/// surowa ścieżka (Alacritty, także ze spacjami), 'pojedyncze' / "podwójne" cudzysłowy
/// (GNOME Terminal, Windows Terminal), znaki ucieczki `\ ` (kitty, Konsole), adresy file://,
/// kilka plików oddzielonych spacją lub nową linią.
pub fn parse_paths(text: &str) -> Vec<PathBuf> {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if text.is_empty() {
        return vec![];
    }
    // Cały tekst jako jedna ścieżka (np. Alacritty wkleja ścieżkę bez cudzysłowów, ze spacjami).
    let whole = PathBuf::from(from_uri(text));
    if whole.exists() {
        return vec![whole];
    }
    // Każda linia osobno: cała linia jako ścieżka (np. kilka plików, każdy w nowej linii),
    // inaczej podział jak w powłoce (cudzysłowy, `\ `).
    let mut out = vec![];
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let p = PathBuf::from(from_uri(line));
        if p.exists() {
            out.push(p);
        } else {
            out.extend(tokens(line).into_iter().map(|t| PathBuf::from(from_uri(&t))).filter(|p| p.exists()));
        }
    }
    out
}

/// Podział jak w powłoce: cudzysłowy, `\` przed znakiem (poza Windows, gdzie `\` to separator).
fn tokens(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut has = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                has = true;
            }
            (None, '\\') if !cfg!(windows) => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    has = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// file:///home/x/a%20b.mkv → /home/x/a b.mkv (Windows: file:///C:/x → C:/x).
fn from_uri(s: &str) -> String {
    let Some(rest) = s.strip_prefix("file://") else { return s.to_string() };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let rest = if cfg!(windows) { rest.trim_start_matches('/') } else { rest };
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn formats() {
        let d = std::env::temp_dir().join(format!("vp-drop-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let a = d.join("film z spacją.mkv");
        let b = d.join("b's.mp4");
        fs::write(&a, b"x").unwrap();
        fs::write(&b, b"x").unwrap();
        let (sa, sb) = (a.display().to_string(), b.display().to_string());

        assert_eq!(parse_paths(&format!("{sa} ")), vec![a.clone()]); // Alacritty
        assert_eq!(parse_paths(&format!("\"{sa}\" \"{sb}\"")), vec![a.clone(), b.clone()]); // WT
        assert_eq!(parse_paths(&format!("'{sa}'")), vec![a.clone()]); // GNOME
        assert_eq!(parse_paths(&sa.replace(' ', "\\ ")), vec![a.clone()]); // kitty/Konsole
        assert_eq!(parse_paths(&format!("file://{}", sa.replace(' ', "%20"))), vec![a.clone()]);
        assert_eq!(parse_paths(&format!("{sa}\n{sb}\n")), vec![a.clone(), b.clone()]);
        assert!(parse_paths("+-*").is_empty());
        fs::remove_dir_all(&d).unwrap();
    }
}
