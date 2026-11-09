//! Czytelne wyjaśnienie, dlaczego nie da się usunąć pliku lub katalogu (F6):
//! kto jest właścicielem, jakich uprawnień brakuje i jak to naprawić.
use std::{io, path::Path};

/// Linie komunikatu dla błędu usuwania `path`.
pub fn explain(path: &Path, err: &io::Error) -> Vec<String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string());
    let is_dir = std::fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    let what = if is_dir { "katalogu" } else { "pliku" };
    let mut out = vec![];

    if err.raw_os_error() == Some(30) {
        // EROFS
        out.push(format!("Nie można usunąć {what} „{name}” — dysk jest zamontowany tylko do odczytu."));
        out.push("Zamontuj go ponownie z prawem zapisu (np. sudo mount -o remount,rw <punkt montowania>).".into());
        return out;
    }
    if err.kind() != io::ErrorKind::PermissionDenied {
        out.push(format!("Nie można usunąć {what} „{name}”: {err}"));
        if err.raw_os_error() == Some(16) {
            out.push("Plik lub katalog jest używany (np. zamontowany albo otwarty przez inny program).".into());
        }
        return out;
    }

    out.push(format!("Nie można usunąć {what} „{name}” — brak uprawnień."));
    out.extend(platform_detail(path, is_dir));
    if is_dir && path.exists() {
        out.push("Część zawartości katalogu mogła już zostać usunięta — reszta jest nietknięta.".into());
    }
    out
}

#[cfg(unix)]
fn platform_detail(path: &Path, is_dir: bool) -> Vec<String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mut out = vec![];
    // Tożsamość procesu bez dodatkowych bibliotek: właściciel /proc/self.
    let (uid, gid) = std::fs::metadata("/proc/self").map(|m| (m.uid(), m.gid())).unwrap_or((u32::MAX, u32::MAX));
    let me = user_name(uid);
    let can_write = |p: &Path| -> Option<bool> {
        let m = std::fs::metadata(p).ok()?;
        let mode = m.mode();
        Some(if uid == 0 {
            true
        } else if m.uid() == uid {
            mode & 0o200 != 0 && mode & 0o100 != 0
        } else if m.gid() == gid {
            mode & 0o020 != 0 && mode & 0o010 != 0
        } else {
            mode & 0o002 != 0 && mode & 0o001 != 0
        })
    };
    let describe = |p: &Path| -> String {
        match std::fs::symlink_metadata(p) {
            Ok(m) => format!(
                "właściciel: {}, uprawnienia: {}",
                user_name(m.uid()),
                mode_string(m.permissions().mode(), m.is_dir())
            ),
            Err(_) => "brak dostępu do informacji".into(),
        }
    };

    // 1. Katalog nadrzędny: usunięcie wpisu wymaga w nim prawa zapisu (i braku blokady sticky bit).
    if let Some(parent) = path.parent() {
        if can_write(parent) == Some(false) {
            out.push(format!("Brak prawa zapisu w katalogu nadrzędnym {} ({}).", parent.display(), describe(parent)));
        } else if let (Ok(pm), Ok(m)) = (std::fs::metadata(parent), std::fs::symlink_metadata(path)) {
            if pm.mode() & 0o1000 != 0 && m.uid() != uid && pm.uid() != uid && uid != 0 {
                out.push(format!(
                    "Katalog nadrzędny {} ma bit „sticky” — usuwać może tylko właściciel ({}).",
                    parent.display(),
                    user_name(m.uid())
                ));
            }
        }
    }
    // 2. Katalog: potrzebny zapis w nim samym i we wszystkich podkatalogach — znajdź pierwszy blokujący.
    if is_dir {
        let mut budget = 50_000u32;
        if let Some(blocker) = first_unwritable_dir(path, &can_write, &mut budget) {
            out.push(format!("Brak prawa zapisu w {} ({}).", blocker.display(), describe(&blocker)));
        }
    }
    if out.is_empty() {
        out.push(format!("Obiekt: {}.", describe(path)));
    }
    let q = shell_quote(path);
    out.push(format!("Program działa jako użytkownik {me}. Możesz:"));
    out.push(format!("  • przejąć własność:  sudo chown -R {me}: {q}"));
    out.push(format!("  • albo usunąć ręcznie:  sudo rm -r{} {q}", if is_dir { "" } else { "f" }));
    out
}

#[cfg(unix)]
fn first_unwritable_dir(
    dir: &Path,
    can_write: &dyn Fn(&Path) -> Option<bool>,
    budget: &mut u32,
) -> Option<std::path::PathBuf> {
    if can_write(dir) == Some(false) {
        return Some(dir.to_path_buf());
    }
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let p = e.path();
        if std::fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false) {
            if let Some(b) = first_unwritable_dir(&p, can_write, budget) {
                return Some(b);
            }
        }
    }
    None
}

#[cfg(unix)]
fn user_name(uid: u32) -> String {
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                let mut f = l.split(':');
                let name = f.next()?;
                let id: u32 = f.nth(1)?.parse().ok()?;
                (id == uid).then(|| name.to_string())
            })
        })
        .unwrap_or_else(|| format!("UID {uid}"))
}

#[cfg(unix)]
fn mode_string(mode: u32, dir: bool) -> String {
    let mut s = String::from(if dir { "d" } else { "-" });
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

#[cfg(unix)]
fn shell_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

#[cfg(windows)]
fn platform_detail(path: &Path, _is_dir: bool) -> Vec<String> {
    let ro = std::fs::metadata(path).map(|m| m.permissions().readonly()).unwrap_or(false);
    let mut out = vec![];
    if ro {
        out.push("Plik ma atrybut „tylko do odczytu” — zdejmij go we Właściwościach pliku.".into());
    }
    out.push("Możliwe przyczyny: plik jest otwarty w innym programie (np. odtwarzaczu), należy do innego".into());
    out.push("użytkownika albo folder chroni system. Zamknij programy używające pliku albo uruchom".into());
    out.push("VideoPipeline jako administrator.".into());
    out
}
