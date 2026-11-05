use crate::{
    analysis::Analysis,
    config::*,
    copy,
    model::{Panel, State},
    proc,
};
use anyhow::{bail, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn run_quiet(cmd: &str, args: &[String]) -> bool {
    proc::run(Command::new(cmd).args(args).stdin(Stdio::null())) == Some(0)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn encode_audio(input: &Path, idx: u32, bitrate: &str, channels: &str, lang: &str, out: &Path) -> bool {
    let args: Vec<String> = [
        "-hide_banner", "-loglevel", "quiet", "-i", &s(input), "-map", &format!("0:{idx}"),
        "-c:a", "eac3", "-b:a", bitrate, "-ac", channels, "-metadata:s:a:0",
        &format!("language={lang}"), &s(out),
    ]
    .iter()
    .map(|x| x.to_string())
    .collect();
    run_quiet("ffmpeg", &args)
}

/// Kopiowanie do source/encoded (tylko z -c) po ukończeniu / przy resume.
fn finish_copy(opts: &Opts, a: &Analysis, output: &Path, already: bool, panel: &mut Panel) {
    let name = &a.filename;
    if !opts.copy {
        let note = if already {
            "Plik wynikowy już istnieje — kopiowanie -c wyłączone"
        } else {
            "Cały pipeline zakończony — kopiowanie -c wyłączone"
        };
        panel.set(name, State::Done, note);
        return;
    }
    match copy::copy_final_to_source_encoded(&a.input_file, output) {
        Ok(()) => {
            panel.set(name, State::Done, "Kopia w encoded zweryfikowana SHA-512");
            plog!(" -> [{name}] GOTOWE! Kopia w encoded: SHA-512 OK.");
        }
        Err(e) => {
            // Sam plik wynikowy jest poprawny; kolejne uruchomienie z -c ponowi kopię.
            panel.set(name, State::Error, "Kopiowanie do encoded nie powiodło się");
            plog!(" -> [{name}] BŁĄD kopiowania do encoded: {e:#}");
        }
    }
}

pub fn run(opts: &Opts, a: &Analysis, analysis_file: &Path, panel: &mut Panel) -> Result<()> {
    let name = &a.filename;
    let p = opts.paths(&a.stem);
    panel.set(name, State::Hdr, "");
    // Opcje wygrywają z wynikiem starej analizy (np. DV wykryte, ale teraz --no-dv).
    let use_dovi = a.has_dovi && !opts.no_dv;
    let use_hdr10plus = a.has_hdr10plus && !opts.no_hdr10plus;
    if proc::cancelled() {
        bail!("przerwano");
    }

    if p.output_mkv.exists() {
        finish_copy(opts, a, &p.output_mkv, true, panel);
        return Ok(());
    }
    plog!(" -> [{name}] Składanie pliku wynikowego ({})...", opts.codec.name());

    // --- wideo + dynamiczne HDR -------------------------------------------
    let current_video: PathBuf = if opts.codec == Codec::Av1 {
        if !p.temp_video.exists() {
            bail!("brak pliku {}", p.temp_video.display());
        }
        panel.set(name, State::Audio, "AV1 — DV/HDR10+ inject pominięte");
        p.temp_video.clone()
    } else {
        let mut cur = if p.step2.exists() {
            plog!("    Wznowienie: znaleziono *_step2.hevc.");
            p.step2.clone()
        } else if p.step1.exists() {
            plog!("    Wznowienie: znaleziono *_step1.hevc.");
            p.step1.clone()
        } else if p.temp_video.exists() {
            p.temp_video.clone()
        } else {
            bail!("brak pliku {}", p.temp_video.display());
        };

        if use_hdr10plus && !p.step2.exists() {
            if p.step1.exists() {
                cur = p.step1.clone();
            } else {
                if !p.temp_hdr10plus.exists() {
                    bail!("brak pliku HDR10+ JSON: {}", p.temp_hdr10plus.display());
                }
                panel.set(name, State::Hdr10Plus, "Wstrzykiwanie HDR10+");
                panel.draw(" ETAP 3/3 — PIPELINE");
                let args = ["inject", "-i", &s(&cur), "-j", &s(&p.temp_hdr10plus), "-o", &s(&p.step1)]
                    .map(String::from);
                if !run_quiet("hdr10plus_tool", &args) {
                    bail!("wstrzykiwanie HDR10+ nie powiodło się");
                }
                cur = p.step1.clone();
            }
        }
        if use_dovi {
            if p.step2.exists() {
                cur = p.step2.clone();
            } else {
                if !p.temp_rpu.exists() {
                    bail!("brak pliku RPU: {}", p.temp_rpu.display());
                }
                panel.set(name, State::Dv, "Wstrzykiwanie Dolby Vision");
                panel.draw(" ETAP 3/3 — PIPELINE");
                let args = ["inject-rpu", "-i", &s(&cur), "--rpu-in", &s(&p.temp_rpu), "-o", &s(&p.step2)]
                    .map(String::from);
                if !run_quiet("dovi_tool", &args) {
                    bail!("wstrzykiwanie Dolby Vision nie powiodło się");
                }
                cur = p.step2.clone();
            }
        }
        panel.set(name, State::Audio, "HDR/DV zakończone lub pominięte");
        cur
    };

    // --- audio -----------------------------------------------------------
    if opts.pl_eac3 {
        if let Some(idx) = a.pol_audio {
            if p.temp_pol_audio.exists() {
                plog!("    Wznowienie: *_pol_192.eac3 już istnieje.");
            } else {
                panel.set(name, State::Audio, "Kompresja audio PL E-AC-3 2.0 192k");
                panel.draw(" ETAP 3/3 — PIPELINE");
                if !encode_audio(&a.input_file, idx, "192k", "2", "pol", &p.temp_pol_audio) {
                    bail!("kompresja polskiej ścieżki E-AC-3 192k nie powiodła się");
                }
            }
        }
    }
    // Angielskie audio kompresujemy i dodajemy tylko z -da.
    if let Some(idx) = a.eng_audio.filter(|_| opts.extra) {
        if p.temp_eng_audio.exists() {
            plog!("    Wznowienie: *_eng.eac3 już istnieje.");
        } else {
            panel.set(name, State::Audio, "Kompresja audio ENG");
            panel.draw(" ETAP 3/3 — PIPELINE");
            if !encode_audio(&a.input_file, idx, "484k", "6", "eng", &p.temp_eng_audio) {
                bail!("kompresja angielskiej ścieżki nie powiodła się");
            }
        }
    }

    // --- mkvmerge --------------------------------------------------------
    panel.set(name, State::Mux, "Łączenie strumieni MKV");
    panel.draw(" ETAP 3/3 — PIPELINE");

    let mut cmd: Vec<String> = vec!["--output".into(), s(&p.output_mkv), s(&current_video)];
    let mut flags: Vec<String> = vec!["--no-video".into(), "--no-buttons".into()];
    let mut has_audio = false;

    if let Some(pol) = a.pol_audio {
        if opts.pl_eac3 {
            flags.push("--no-audio".into()); // PL dodany osobno jako E-AC-3
        } else {
            flags.extend(["--audio-tracks".into(), pol.to_string()]);
            has_audio = true;
        }
    } else if a.first_audio.is_some() && (!opts.extra || a.eng_audio.is_none()) {
        // Brak ścieżki PL: zachowujemy pierwszą ścieżkę audio (żeby plik nie był niemy).
        flags.extend(["--audio-tracks".into(), a.first_audio.unwrap().to_string()]);
        has_audio = true;
    } else {
        flags.push("--no-audio".into());
    }

    // Napisy EN i PL tylko z -da.
    let subs: Vec<String> = if opts.extra {
        [a.pol_sub, a.eng_sub].iter().flatten().map(|x| x.to_string()).collect()
    } else {
        Vec::new()
    };
    let has_subs = !subs.is_empty();
    if has_subs {
        flags.extend(["--subtitle-tracks".into(), subs.join(",")]);
    } else {
        flags.push("--no-subtitles".into());
    }

    if has_audio || has_subs {
        cmd.extend(flags);
        cmd.push(s(&a.input_file));
    }
    if opts.pl_eac3 && a.pol_audio.is_some() && p.temp_pol_audio.exists() {
        cmd.extend(["--language".into(), "0:pol".into(), s(&p.temp_pol_audio)]);
    }
    if opts.extra && p.temp_eng_audio.exists() {
        cmd.extend(["--language".into(), "0:eng".into(), s(&p.temp_eng_audio)]);
    }

    // mkvmerge zwraca 1 dla samych ostrzeżeń — to nadal sukces.
    let ok = proc::run(Command::new("mkvmerge").args(&cmd).stdin(Stdio::null())).map_or(false, |c| c <= 1);
    if !ok {
        bail!("błąd mkvmerge — pliki tymczasowe pozostają, kolejne uruchomienie wznowi proces");
    }

    // Pliki tymczasowe usuwamy dopiero po udanym MKV.
    for f in [
        &p.temp_video, &p.step1, &p.step2, &p.temp_rpu, &p.temp_hdr10plus,
        &p.temp_eng_audio, &p.temp_pol_audio, &analysis_file.to_path_buf(),
    ] {
        let _ = fs::remove_file(f);
    }

    finish_copy(opts, a, &p.output_mkv, false, panel);
    Ok(())
}
