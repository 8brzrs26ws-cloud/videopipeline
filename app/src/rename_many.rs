//! F4 dla wielu plików: zamiana fragmentów nazw (np. „{film}.mkv” → „film.mkv”) w zaznaczonych
//! plikach albo we wszystkich plikach katalogu o danym rozszerzeniu, z podglądem przed zmianą.
use crate::app::TextInput;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub const R_FIND: usize = 0;
pub const R_REPLACE: usize = 1;
pub const R_EXT: usize = 2;
pub const R_WITH_EXT: usize = 3;
pub const R_FIELDS: usize = 4;

pub struct Plan {
    pub from: PathBuf,
    pub old: String,
    pub new: String,
    /// Powód, dla którego tej zmiany nie da się wykonać (nazwa zajęta, niepoprawna…).
    pub problem: Option<String>,
}

pub struct RenameMany {
    pub dir: PathBuf,
    /// Zaznaczone pliki z lewego panelu; puste = wszystkie pliki katalogu (z filtrem rozszerzenia).
    pub marked: Vec<PathBuf>,
    pub find: TextInput,
    pub replace: TextInput,
    pub ext: TextInput,
    /// Zamiana także w rozszerzeniu (np. „.MP4” → „.mp4”).
    pub with_ext: bool,
    pub field: usize,
    pub plans: Vec<Plan>,
    /// Plików w zakresie (przed zamianą).
    pub candidates: usize,
    pub scroll: usize,
    pub page: usize,
}

fn input(s: &str) -> TextInput {
    TextInput { chars: s.chars().collect(), cursor: s.chars().count() }
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

impl RenameMany {
    pub fn new(dir: PathBuf, marked: Vec<PathBuf>, ext: &str) -> Self {
        let mut r = RenameMany {
            dir,
            marked,
            find: input(""),
            replace: input(""),
            ext: input(ext),
            with_ext: false,
            field: R_FIND,
            plans: vec![],
            candidates: 0,
            scroll: 0,
            page: 10,
        };
        r.update();
        r
    }

    pub fn input_mut(&mut self) -> Option<&mut TextInput> {
        match self.field {
            R_FIND => Some(&mut self.find),
            R_REPLACE => Some(&mut self.replace),
            R_EXT => Some(&mut self.ext),
            _ => None,
        }
    }

    /// Pliki w zakresie: zaznaczone albo wszystkie pliki katalogu; filtr rozszerzenia (bez kropki,
    /// bez rozróżniania wielkości liter, puste = wszystkie).
    fn scope(&self) -> Vec<PathBuf> {
        let ext = self.ext.text().trim().trim_start_matches('.').to_lowercase();
        let all: Vec<PathBuf> = if self.marked.is_empty() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&self.dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            v.sort();
            v
        } else {
            self.marked.iter().filter(|p| p.is_file()).cloned().collect()
        };
        all.into_iter()
            .filter(|p| {
                ext.is_empty()
                    || p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.to_lowercase() == ext)
            })
            .collect()
    }

    /// Nowa nazwa: każdy wariant z pola „Szukaj” (oddzielone „|”) zamieniony na tekst z „Zamień na”.
    fn apply(&self, name: &str) -> String {
        let find = self.find.text();
        let to = self.replace.text();
        let pats: Vec<&str> = find.split('|').filter(|p| !p.is_empty()).collect();
        let (stem, ext) = if self.with_ext { (name, "") } else { split_ext(name) };
        let mut s = stem.to_string();
        for p in pats {
            s = s.replace(p, &to);
        }
        format!("{s}{ext}")
    }

    /// Przelicza podgląd po każdej zmianie pól.
    pub fn update(&mut self) {
        let files = self.scope();
        self.candidates = files.len();
        let mut plans: Vec<Plan> = vec![];
        for from in files {
            let old = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let new = self.apply(&old);
            if new != old {
                plans.push(Plan { from, old, new, problem: None });
            }
        }
        // Konflikty: ta sama nowa nazwa dla kilku plików, nazwa zajęta przez plik spoza zmian,
        // nazwa niepoprawna. Windows nie rozróżnia wielkości liter — porównujemy bez niej.
        let key = |s: &str| if cfg!(windows) { s.to_lowercase() } else { s.to_string() };
        let mut count: HashMap<(PathBuf, String), usize> = HashMap::new();
        for p in &plans {
            *count.entry((p.from.parent().unwrap_or(Path::new("")).to_path_buf(), key(&p.new))).or_default() += 1;
        }
        let renamed: Vec<String> = plans.iter().map(|p| key(&p.old)).collect();
        for p in plans.iter_mut() {
            let dir = p.from.parent().unwrap_or(Path::new("")).to_path_buf();
            let target = dir.join(&p.new);
            p.problem = if p.new.trim().is_empty() || p.new == "." || p.new == ".." {
                Some("pusta nazwa".into())
            } else if p.new.contains('/') || (cfg!(windows) && p.new.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])) {
                Some("niedozwolony znak".into())
            } else if count.get(&(dir, key(&p.new))).copied().unwrap_or(0) > 1 {
                Some("ta sama nazwa dla kilku plików".into())
            } else if target.exists() && key(&p.new) != key(&p.old) && !renamed.contains(&key(&p.new)) {
                Some("nazwa zajęta".into())
            } else {
                None
            };
        }
        self.plans = plans;
        self.scroll = self.scroll.min(self.plans.len().saturating_sub(1));
    }

    pub fn conflicts(&self) -> usize {
        self.plans.iter().filter(|p| p.problem.is_some()).count()
    }

    /// Wykonuje zmiany bez konfliktów. Zwraca (stara ścieżka, nowa ścieżka) udanych i błędy.
    /// Nowa nazwa zajęta przez inny zmieniany plik (zamiana a↔b) — przez nazwę tymczasową.
    pub fn execute(&self) -> (Vec<(PathBuf, PathBuf)>, Vec<String>) {
        let ok: Vec<&Plan> = self.plans.iter().filter(|p| p.problem.is_none()).collect();
        let mut staged: Vec<(PathBuf, PathBuf, PathBuf)> = vec![];
        let mut errors = vec![];
        for (i, p) in ok.iter().enumerate() {
            let tmp = p.from.with_file_name(format!(".vp-rename-{}-{i}.tmp", std::process::id()));
            match std::fs::rename(&p.from, &tmp) {
                Ok(()) => staged.push((p.from.clone(), tmp, p.from.with_file_name(&p.new))),
                Err(e) => errors.push(format!("{}: {e}", p.old)),
            }
        }
        let mut done = vec![];
        for (from, tmp, to) in staged {
            let res = if to.exists() { Err(std::io::Error::other("nazwa zajęta")) } else { std::fs::rename(&tmp, &to) };
            match res {
                Ok(()) => done.push((from, to)),
                Err(e) => {
                    let _ = std::fs::rename(&tmp, &from);
                    errors.push(format!("{}: {e}", from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                }
            }
        }
        (done, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_and_execute() {
        let d = std::env::temp_dir().join(format!("vp-ren-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in ["{film}.mkv", "{inny}.MKV", "{x}.mp4", "a.mkv", "b.mkv"] {
            std::fs::write(d.join(f), f).unwrap();
        }
        let mut r = RenameMany::new(d.clone(), vec![], "mkv");
        assert_eq!(r.candidates, 4);
        r.find = input("{|}");
        r.update();
        let mut news: Vec<&str> = r.plans.iter().map(|p| p.new.as_str()).collect();
        news.sort();
        assert_eq!(news, ["film.mkv", "inny.MKV"]);
        let (done, err) = r.execute();
        assert_eq!((done.len(), err.len()), (2, 0));
        assert!(d.join("film.mkv").exists() && d.join("{x}.mp4").exists());

        // a ↔ b przez nazwę tymczasową; kolizja z plikiem spoza zmian.
        let mut r = RenameMany::new(d.clone(), vec![d.join("a.mkv")], "");
        r.find = input("a");
        r.replace = input("b");
        r.update();
        assert_eq!(r.plans[0].problem.as_deref(), Some("nazwa zajęta"));
        r.with_ext = true;
        r.find = input(".mkv");
        r.replace = input(".MKV");
        r.update();
        assert_eq!(r.plans[0].new, "a.MKV");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
