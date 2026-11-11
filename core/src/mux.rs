use crate::{
    analysis::Analysis,
    config::*,
    copy,
    model::{Panel, State},
    probe, proc,
};
use anyhow::{bail, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

fn run_quiet(cmd: &str, args: &[String]) -> bool {
    proc::run(crate::proc::command(cmd).args(args).stdin(Stdio::null())) == Some(0)
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

/// Które strumienie źródła kopiujemy bez zmian (wspólne dla MKV i MP4).
fn source_audio(opts: &Opts, a: &Analysis) -> Vec<u32> {
    if let Some(pol) = a.pol_audio {
        // Z -pleac3 polska ścieżka jest dodawana osobno jako E-AC-3.
        if opts.pl_eac3 { vec![] } else { vec![pol] }
    } else if let Some(first) = a.first_audio.filter(|_| !opts.extra || a.eng_audio.is_none()) {
        // Brak ścieżki PL: zachowujemy pierwszą ścieżkę audio (żeby plik nie był niemy).
        vec![first]
    } else {
        vec![]
    }
}

/// Napisy EN i PL tylko z -da.
fn source_subs(opts: &Opts, a: &Analysis) -> Vec<u32> {
    if opts.extra {
        [a.pol_sub, a.eng_sub].iter().flatten().copied().collect()
    } else {
        vec![]
    }
}

fn mux_mkv(opts: &Opts, a: &Analysis, p: &Paths, current_video: &Path) -> Result<()> {
    let mut cmd: Vec<String> = vec!["--output".into(), s(&p.output), s(current_video)];
    let mut flags: Vec<String> = vec!["--no-video".into(), "--no-buttons".into()];

    let audio = source_audio(opts, a);
    if audio.is_empty() {
        flags.push("--no-audio".into());
    } else {
        let ids: Vec<String> = audio.iter().map(|x| x.to_string()).collect();
        flags.extend(["--audio-tracks".into(), ids.join(",")]);
    }

    let subs = source_subs(opts, a);
    if subs.is_empty() {
        flags.push("--no-subtitles".into());
    } else {
        let ids: Vec<String> = subs.iter().map(|x| x.to_string()).collect();
        flags.extend(["--subtitle-tracks".into(), ids.join(",")]);
    }

    if !audio.is_empty() || !subs.is_empty() {
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
    let ok = proc::run(crate::proc::command("mkvmerge").args(&cmd).stdin(Stdio::null())).map_or(false, |c| c <= 1);
    if !ok {
        bail!("błąd mkvmerge — pliki tymczasowe pozostają, kolejne uruchomienie wznowi proces");
    }
    Ok(())
}

/// MP4 (źródło .mp4): składanie przez ffmpeg. mkvmerge nie zapisuje MP4.
/// Strumienie źródła są kopiowane (źródło MP4 ma już audio i napisy zgodne z MP4).
fn mux_mp4(opts: &Opts, a: &Analysis, p: &Paths, video: &Path, dynamic_hdr: bool) -> Result<()> {
    // Surowy strumień HEVC (po dovi_tool/hdr10plus_tool) nie ma znaczników czasu — podajemy fps ze źródła.
    let raw_hevc = video.extension().and_then(|e| e.to_str()) == Some("hevc");

    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y"].map(String::from).to_vec();
    if raw_hevc {
        let Some(fps) = probe::video_fps(&a.input_file) else {
            bail!("nie można odczytać liczby klatek na sekundę ze źródła (ffprobe)");
        };
        args.extend(["-framerate".into(), fps, "-f".into(), "hevc".into()]);
    }
    args.extend(["-i".into(), s(video), "-i".into(), s(&a.input_file)]);

    let mut maps: Vec<String> = vec!["-map".into(), "0:v:0".into()];
    let mut meta: Vec<String> = vec![];
    let mut out_audio = 0usize;
    let mut next_input = 2usize; // 0 = wideo, 1 = źródło

    for idx in source_audio(opts, a) {
        maps.extend(["-map".into(), format!("1:{idx}")]);
        out_audio += 1;
    }
    // E-AC-3 z plików tymczasowych: surowy strumień nie niesie języka, więc ustawiamy go jawnie.
    let mut temp_audio: Vec<(&PathBuf, &str)> = vec![];
    if opts.pl_eac3 && a.pol_audio.is_some() && p.temp_pol_audio.exists() {
        temp_audio.push((&p.temp_pol_audio, "pol"));
    }
    if opts.extra && p.temp_eng_audio.exists() {
        temp_audio.push((&p.temp_eng_audio, "eng"));
    }
    for (path, lang) in temp_audio {
        args.extend(["-i".into(), s(path)]);
        maps.extend(["-map".into(), format!("{next_input}:0")]);
        meta.extend([format!("-metadata:s:a:{out_audio}"), format!("language={lang}")]);
        out_audio += 1;
        next_input += 1;
    }
    for idx in source_subs(opts, a) {
        maps.extend(["-map".into(), format!("1:{idx}")]);
    }

    args.extend(maps);
    args.extend(meta);
    args.extend(["-map_chapters".into(), "1".into(), "-map_metadata".into(), "1".into()]);
    args.extend(["-c".into(), "copy".into()]);
    if opts.codec == Codec::Hevc {
        // hvc1: zgodność z odtwarzaczami (Apple, przeglądarki, KDE).
        args.extend(["-tag:v".into(), "hvc1".into()]);
    }
    args.extend(["-movflags".into(), "+faststart".into(), "-f".into(), "mp4".into()]);

    // Zapis do pliku tymczasowego: przerwany mux nie może zostać uznany za gotowy wynik.
    let part = PathBuf::from(format!("{}.part", p.output.display()));
    let _ = fs::remove_file(&part);
    args.push(s(&part));

    if dynamic_hdr {
        plog!("    UWAGA (MP4): RPU Dolby Vision i metadane HDR10+ zostają w strumieniu, ale ffmpeg nie zapisuje konfiguracji DV (dvcC) — część odtwarzaczy pokaże zwykły HDR10.");
    }
    if !run_quiet("ffmpeg", &args) {
        let _ = fs::remove_file(&part);
        bail!("błąd ffmpeg przy składaniu MP4 — pliki tymczasowe pozostają, kolejne uruchomienie wznowi proces");
    }
    fs::rename(&part, &p.output)?;
    Ok(())
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
    let p = opts.paths_for(&a.input_file);
    panel.set(name, State::Hdr, "");
    // Opcje wygrywają z wynikiem starej analizy (np. DV wykryte, ale teraz --no-dv).
    let use_dovi = a.has_dovi && !opts.no_dv;
    let use_hdr10plus = a.has_hdr10plus && !opts.no_hdr10plus;
    if proc::cancelled() {
        bail!("przerwano");
    }

    if p.output.exists() {
        finish_copy(opts, a, &p.output, true, panel);
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

    // --- składanie kontenera ------------------------------------------------
    panel.set(name, State::Mux, format!("Łączenie strumieni ({})", p.container.ext().to_uppercase()));
    panel.draw(" ETAP 3/3 — PIPELINE");

    match p.container {
        Container::Mp4 => mux_mp4(opts, a, &p, &current_video, use_dovi || use_hdr10plus)?,
        Container::Mkv => mux_mkv(opts, a, &p, &current_video)?,
    }

    // Pliki tymczasowe usuwamy dopiero po udanym złożeniu pliku.
    for f in [
        &p.temp_video, &p.step1, &p.step2, &p.temp_rpu, &p.temp_hdr10plus,
        &p.temp_eng_audio, &p.temp_pol_audio, &analysis_file.to_path_buf(),
    ] {
        let _ = fs::remove_file(f);
    }

    finish_copy(opts, a, &p.output, false, panel);
    Ok(())
}
