//! Wyszukiwanie plików wideo (używane przez TUI i tryb skryptowy).
use anyhow::{bail, Result};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

const VIDEO_EXT: [&str; 5] = ["mp4", "mkv", "mov", "avi", "m2ts"];

pub fn is_video(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Pliki wideo z katalogu (bez rekurencji), posortowane.
pub fn videos_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_video(p))
        .collect();
    v.sort();
    v
}

/// Rozwija listę plików/katalogów do listy plików wideo.
/// Pipeline kluczuje stan po nazwie pliku, więc duplikaty nazw są pomijane.
pub fn discover(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = vec![];
    let mut names = HashSet::new();
    for p in paths {
        let list = if p.is_dir() {
            videos_in(p)
        } else if p.is_file() {
            vec![p.clone()]
        } else {
            bail!("Podana ścieżka '{}' nie istnieje lub jest nieprawidłowa.", p.display());
        };
        for f in list {
            let f = f.canonicalize().unwrap_or(f);
            if out.contains(&f) {
                continue;
            }
            if let Some(n) = f.file_name() {
                if !names.insert(n.to_os_string()) {
                    plog!("UWAGA: pomijam {} — plik o tej samej nazwie już jest w kolejce", f.display());
                    continue;
                }
            }
            out.push(f);
        }
    }
    Ok(out)
}
