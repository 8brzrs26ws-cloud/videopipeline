use crate::{
    analysis::Analysis,
    config::*,
    model::{read_progress, Panel, State},
    probe,
    proc::{self, PIDS},
};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::Duration,
};

struct Running {
    child: Child,
    name: String,
    output: PathBuf,
    log: PathBuf,
    pf: PathBuf,
}

fn sv(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

fn build_args(
    opts: &Opts,
    rt: &Runtime,
    a: &Analysis,
    out: &Path,
    pf: &Path,
) -> (Vec<String>, String) {
    let c = &a.color;
    let mut hw: Vec<String> = vec![];
    let mut fmt: Vec<String> = vec![];
    let mut filt: Vec<String> = vec![];
    let note;

    if c.is_sdr_8bit {
        if opts.codec == Codec::Hevc {
            fmt.extend(sv(&["-profile:v", "main"]));
        }
        fmt.extend(sv(&["-pix_fmt", "yuv420p"]));
        fmt.extend(sv(&["-color_primaries", &c.sdr_primaries, "-color_trc", &c.sdr_trc]));
        fmt.extend(sv(&["-colorspace", &c.sdr_space, "-color_range", &c.sdr_range]));

        if opts.denoise && rt.denoise != Denoise::Off {
            let mut chain: Vec<String> = vec![];
            if let Some(cr) = &a.crop {
                chain.push(cr.clone());
            }
            if rt.denoise == Denoise::Cuda {
                chain.push(format!(
                    "format=yuv420p,hwupload_cuda,{DENOISE_CUDA_FILTER},hwdownload,format=yuv420p"
                ));
                hw = sv(&["-init_hw_device", "cuda=denoise:0", "-filter_hw_device", "denoise"]);
                note = "SDR 8-bit + CUDA denoise";
            } else {
                chain.push(DENOISE_CPU_FILTER.into());
                note = "SDR 8-bit + CPU denoise";
            }
            filt = vec!["-vf".into(), chain.join(",")];
        } else {
            if let Some(cr) = &a.crop {
                filt = vec!["-vf".into(), cr.clone()];
            }
            note = if opts.codec == Codec::Av1 {
                "SDR 8-bit -> AV1 10-bit (highbitdepth)"
            } else {
                "SDR 8-bit"
            };
        }
    } else {
        if opts.codec == Codec::Hevc {
            fmt.extend(sv(&["-profile:v", "main10"]));
        }
        fmt.extend(sv(&["-pix_fmt", "p010le", "-color_primaries", "bt2020"]));
        fmt.extend(sv(&["-color_trc", "smpte2084", "-colorspace", "bt2020nc"]));
        if let Some(cr) = &a.crop {
            filt = vec!["-vf".into(), cr.clone()];
        }
        note = if opts.denoise {
            "HDR — denoise pominięty (tylko SDR 8-bit)"
        } else {
            "HDR 10-bit"
        };
    }

    let cq = opts.cq().to_string();
    let enc: Vec<String> = match opts.codec {
        Codec::Av1 => sv(&[
            "-c:v", "av1_nvenc", "-preset", "p7", "-tune", &rt.av1_tune, "-rc", "vbr",
            "-cq", &cq, "-b:v", "0", "-spatial-aq", "1", "-temporal-aq", "1", "-bf", "4",
            "-b_ref_mode", "middle", "-multipass", "fullres", "-rc-lookahead", "32",
            "-highbitdepth", "1",
        ]),
        Codec::Hevc => sv(&[
            "-c:v", "hevc_nvenc", "-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq",
            &cq, "-b:v", "0", "-spatial-aq", "1", "-temporal-aq", "1", "-b_ref_mode",
            "middle", "-multipass", "2", "-rc-lookahead", "32",
        ]),
    };

    let mut v = sv(&["-hide_banner", "-nostats", "-progress"]);
    v.push(pf.to_string_lossy().into());
    v.extend(hw);
    // Oba równoległe enkodery dekodują przez CUDA/NVDEC.
    v.extend(sv(&["-hwaccel", "cuda"]));
    v.push("-i".into());
    v.push(a.input_file.to_string_lossy().into());
    v.extend(filt);
    v.extend(sv(&["-an", "-sn", "-map", "0:v:0"]));
    v.extend(enc);
    v.extend(fmt);
    v.push(out.to_string_lossy().into());
    (v, note.to_string())
}

/// Sukces = niepusty plik + progress=end + właściwy kodek.
fn result_valid(opts: &Opts, out: &Path, pf: &Path) -> bool {
    if !fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false) {
        return false;
    }
    if !read_progress(pf).ended {
        return false;
    }
    probe::video_codec(out).as_deref() == Some(opts.codec.name())
}

/// HEVC: stara semantyka V9 (istnienie pliku). AV1: kodek + długość ≈ źródło.
fn existing_temp_valid(opts: &Opts, out: &Path, input: &Path) -> bool {
    if !fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false) {
        return false;
    }
    if opts.codec != Codec::Av1 {
        return true;
    }
    if probe::video_codec(out).as_deref() != Some("av1") {
        return false;
    }
    match (probe::container_duration(input), probe::container_duration(out)) {
        (Some(s), Some(o)) => o + (s * 0.005).max(2.0) >= s,
        _ => true,
    }
}

fn start(
    opts: &Opts,
    rt: &Runtime,
    a: &Analysis,
    slot: usize,
    panel: &Panel,
) -> std::io::Result<Running> {
    let p = opts.paths(&a.stem);
    let key = sanitize(&a.filename);
    let pf = rt.progress_dir.join(format!("{key}.ffprogress"));
    let log = rt.progress_dir.join(format!("{key}.log"));
    let _ = fs::remove_file(&pf);
    File::create(&pf)?;

    let dur_us = probe::container_duration(&a.input_file).map(|d| d * 1e6);
    panel.set_progress(&a.filename, pf.clone(), dur_us);

    let (args, note) = build_args(opts, rt, a, &p.temp_video, &pf);
    let label = "CUDA/NVDEC";
    panel.set(&a.filename, State::Nvenc, format!("NVENC {slot}/2 — {note} — dekoder: {label}"));

    let logf = File::create(&log)?;
    let child = Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(logf.try_clone()?))
        .stderr(Stdio::from(logf))
        .spawn()?;
    proc::register(child.id());
    Ok(Running {
        child,
        name: a.filename.clone(),
        output: p.temp_video,
        log,
        pf,
    })
}

fn finish(opts: &Opts, r: Running, status: ExitStatus, panel: &Panel) {
    proc::unregister(r.child.id());

    if proc::cancelled() {
        // Częściowy plik nie może zostać uznany za ukończony etap przy resume.
        let _ = fs::remove_file(&r.output);
        panel.set(&r.name, State::Waiting, "Przerwano");
        return;
    }
    if status.success() && result_valid(opts, &r.output, &r.pf) {
        panel.set(&r.name, State::NvencDone, "NVENC zakończony i zweryfikowany");
        plog!(" -> ffmpeg [{}]: ZAKOŃCZONY + PLIK OK", r.name);
    } else {
        panel.set(&r.name, State::Error, format!("NVENC nie utworzył poprawnego pliku ({status})"));
        plog!(" -> ffmpeg [{}]: BŁĄD kodowania ({status})", r.name);
        if let Ok(log) = fs::read_to_string(&r.log) {
            let lines: Vec<&str> = log.lines().collect();
            let from = lines.len().saturating_sub(12);
            for l in &lines[from..] {
                plog!("    {l}");
            }
        }
        let _ = fs::remove_file(&r.output);
    }
}

fn reap(opts: &Opts, running: &mut Vec<Running>, panel: &Panel) {
    let mut i = 0;
    while i < running.len() {
        match running[i].child.try_wait() {
            Ok(Some(status)) => {
                let r = running.remove(i);
                finish(opts, r, status, panel);
            }
            Ok(None) => i += 1,
            Err(_) => {
                running.remove(i);
            }
        }
    }
}

/// Przerwanie: zabijamy ffmpeg i kasujemy niedokończone pliki.
fn abort_all(running: &mut Vec<Running>, panel: &Panel) {
    for r in running.iter_mut() {
        let _ = r.child.kill();
        let _ = r.child.wait();
        proc::unregister(r.child.id());
        let _ = fs::remove_file(&r.output);
        panel.set(&r.name, State::Waiting, "Przerwano");
    }
    running.clear();
    if let Ok(mut g) = PIDS.lock() {
        g.clear();
    }
}

/// Dwa równoległe sloty NVENC, oba z dekodowaniem CUDA/NVDEC.
pub fn run_stage(opts: &Opts, rt: &Runtime, files: &[Analysis], panel: &Panel) {
    panel.draw("ETAP 2/3 — DUAL NVENC");
    let mut running: Vec<Running> = vec![];

    for a in files {
        if proc::cancelled() {
            abort_all(&mut running, panel);
            return;
        }
        let p = opts.paths(&a.stem);
        if p.output_mkv.exists() {
            panel.set(&a.filename, State::Done, "Plik wynikowy już istnieje");
            continue;
        }
        if p.temp_video.exists() {
            if existing_temp_valid(opts, &p.temp_video, &a.input_file) {
                panel.set(&a.filename, State::NvencDone, "NVENC pominięty — istnieje zweryfikowany plik tymczasowy");
                continue;
            }
            plog!("    UWAGA: usuwam niekompletny plik tymczasowy: {}", p.temp_video.display());
            let _ = fs::remove_file(&p.temp_video);
        }

        while running.len() >= 2 {
            reap(opts, &mut running, panel);
            if proc::cancelled() {
                abort_all(&mut running, panel);
                return;
            }
            if running.len() >= 2 {
                thread::sleep(Duration::from_millis(500));
            }
        }

        let slot = running.len() + 1;
        match start(opts, rt, a, slot, panel) {
            Ok(r) => {
                plog!(" -> [{}] PID {} — start", a.filename, r.child.id());
                running.push(r);
            }
            Err(e) => {
                panel.set(&a.filename, State::Error, format!("nie można uruchomić ffmpeg: {e}"));
            }
        }
        thread::sleep(Duration::from_millis(300));
    }

    while !running.is_empty() {
        reap(opts, &mut running, panel);
        if proc::cancelled() {
            abort_all(&mut running, panel);
            return;
        }
        if !running.is_empty() {
            thread::sleep(Duration::from_millis(500));
        }
    }
}
