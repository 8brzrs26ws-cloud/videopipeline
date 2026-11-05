use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha512};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Maksymalna liczba prób kopiowania + weryfikacji SHA-512.
const MAX_ATTEMPTS: u32 = 3;
/// Plik zostawiany w katalogu encoded, gdy wszystkie próby zawiodły.
const ERROR_MARKER: &str = "file_transfer_error";

pub fn sha512(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("otwieranie {}", path.display()))?;
    let mut h = Sha512::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024]; // odczyt sekwencyjny — dobry dla HDD/SMB
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

fn mtime(p: &Path) -> Option<u64> {
    fs::metadata(p)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

fn size(p: &Path) -> u64 {
    fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn write_sidecar(sidecar: &Path, sha: &str, dest: &Path, size: u64, mtime: u64) -> Result<()> {
    let base = dest.file_name().unwrap().to_string_lossy();
    let mut f = File::create(sidecar)?;
    writeln!(f, "{sha}  {base}")?;
    writeln!(f, "# size={size}")?;
    writeln!(f, "# mtime={mtime}")?;
    Ok(())
}

/// (sha, size, mtime) z pliku *.sha512 — kompatybilne z formatem skryptu fish.
fn read_sidecar(path: &Path) -> Option<(String, u64, u64)> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let sha = lines.next()?.split_whitespace().next()?.to_string();
    let (mut size, mut mt) = (None, None);
    for l in lines {
        if let Some(v) = l.strip_prefix("# size=") {
            size = v.trim().parse().ok();
        }
        if let Some(v) = l.strip_prefix("# mtime=") {
            mt = v.trim().parse().ok();
        }
    }
    Some((sha, size?, mt?))
}

/// Jedna próba: kopiowanie + kontrola rozmiaru + pełne porównanie SHA-512.
fn try_copy_once(output: &Path, dest: &Path, src_size: u64, src_sha: &str) -> Result<()> {
    fs::copy(output, dest).with_context(|| format!("kopiowanie do {}", dest.display()))?;

    let dsize = size(dest);
    if dsize != src_size {
        bail!("rozmiar kopii ({dsize} B) nie zgadza się ze źródłem ({src_size} B)");
    }
    plog!("    Kopiowanie zakończone. Weryfikuję SHA-512 kopii...");
    let dsha = sha512(dest)?;
    if dsha != src_sha {
        bail!("niezgodność SHA-512: źródło {src_sha}, kopia {dsha}");
    }
    Ok(())
}

fn marker_is_for(marker: &Path, name: &str) -> bool {
    fs::read_to_string(marker)
        .ok()
        .and_then(|t| t.lines().next().map(|l| l == format!("plik: {name}")))
        .unwrap_or(false)
}

pub fn copy_final_to_source_encoded(input: &Path, output: &Path) -> Result<()> {
    if !output.is_file() {
        bail!("brak gotowego pliku do skopiowania: {}", output.display());
    }
    let dir = input.parent().unwrap_or(Path::new(".")).join("encoded");
    fs::create_dir_all(&dir).with_context(|| format!("tworzenie {}", dir.display()))?;
    let name = output.file_name().unwrap().to_string_lossy().to_string();
    let dest = dir.join(&name);
    let sidecar = dir.join(format!("{name}.sha512"));
    let marker = dir.join(ERROR_MARKER);

    plog!("    SHA-512: obliczam sumę pliku wynikowego...");
    let src_sha = sha512(output)?;
    let src_size = size(output);

    // Szybka ścieżka resume: zweryfikowana kopia, rozmiar i mtime bez zmian.
    if dest.is_file() {
        if let Some((rsha, rsize, rmt)) = read_sidecar(&sidecar) {
            let dsize = size(&dest);
            if Some(rmt) == mtime(&dest) && dsize == src_size && dsize == rsize && rsha == src_sha {
                plog!("    Kopia już zweryfikowana SHA-512 — pomijam pełny odczyt.");
                return Ok(());
            }
        }
        // Kopia bez aktualnego sidecara: najpierw weryfikacja, żeby nie kopiować drugi raz.
        if size(&dest) == src_size {
            plog!("    Istnieje kopia bez aktualnej weryfikacji — sprawdzam SHA-512...");
            if sha512(&dest).ok().as_deref() == Some(src_sha.as_str()) {
                write_sidecar(&sidecar, &src_sha, &dest, src_size, mtime(&dest).unwrap_or(0))?;
                if marker_is_for(&marker, &name) {
                    let _ = fs::remove_file(&marker);
                }
                plog!("    SHA-512 OK — istniejąca kopia jest identyczna.");
                return Ok(());
            }
            plog!("    Istniejąca kopia ma inną sumę — zostanie skopiowana ponownie.");
        } else {
            plog!("    Istniejąca kopia ma inny rozmiar — zostanie skopiowana ponownie.");
        }
    }

    // Do MAX_ATTEMPTS prób: kopia -> SHA-512 -> przy niezgodności kasujemy kopię i od nowa.
    let mut last_err = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let _ = fs::remove_file(&sidecar);
        let _ = fs::remove_file(&dest);
        plog!("    Kopiowanie — próba {attempt}/{MAX_ATTEMPTS}: {}", dest.display());

        match try_copy_once(output, &dest, src_size, &src_sha) {
            Ok(()) => {
                write_sidecar(&sidecar, &src_sha, &dest, size(&dest), mtime(&dest).unwrap_or(0))?;
                if marker_is_for(&marker, &name) {
                    let _ = fs::remove_file(&marker);
                }
                plog!("    SHA-512 OK — kopia jest identyczna (próba {attempt}/{MAX_ATTEMPTS}).");
                plog!("    Suma kontrolna: {}", sidecar.display());
                return Ok(());
            }
            Err(e) => {
                last_err = format!("{e:#}");
                plog!("    BŁĄD (próba {attempt}/{MAX_ATTEMPTS}): {last_err}");
                plog!("    Usuwam przeniesiony plik.");
                let _ = fs::remove_file(&dest);
                let _ = fs::remove_file(&sidecar);
            }
        }
    }

    // Wszystkie próby zawiodły: kopia i jej suma są już usunięte, zostawiamy znacznik błędu.
    let when = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = fs::write(
        &marker,
        format!(
            "plik: {name}\nźródło: {}\nsha512 źródła: {src_sha}\nprób: {MAX_ATTEMPTS}\nostatni błąd: {last_err}\nczas (unix): {when}\n",
            output.display()
        ),
    );
    bail!(
        "transfer nie powiódł się po {MAX_ATTEMPTS} próbach — usunięto kopię, utworzono {}",
        marker.display()
    )
}
