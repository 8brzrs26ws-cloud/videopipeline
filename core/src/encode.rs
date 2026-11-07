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

fn build_args_nvenc(
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


/// "crop=W:H:X:Y" → (W, H, X, Y)
fn parse_crop(s: &str) -> Option<(u32, u32, u32, u32)> {
    let v: Vec<u32> = s.strip_prefix("crop=")?.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    (v.len() == 4).then(|| (v[0], v[1], v[2], v[3]))
}

/// Dekoder QSV dla kodeka źródła (nazwa z ffprobe), jeśli ffmpeg go ma.
fn qsv_decoder(rt: &Runtime, src_codec: Option<&str>) -> Option<String> {
    let name = match src_codec? {
        "h264" => "h264_qsv",
        "hevc" => "hevc_qsv",
        "av1" => "av1_qsv",
        "vp9" => "vp9_qsv",
        "vp8" => "vp8_qsv",
        "mpeg2video" => "mpeg2_qsv",
        "vc1" => "vc1_qsv",
        "mjpeg" => "mjpeg_qsv",
        _ => return None,
    };
    rt.qsv_decoders.iter().any(|d| d == name).then(|| name.to_string())
}

/// Intel: QSV (dekodowanie QSV + vpp_qsv) albo VAAPI (dekodowanie VAAPI + filtry VAAPI).
/// Zwraca (argumenty, opis, dekoder).
fn build_args_intel(
    opts: &Opts,
    rt: &Runtime,
    a: &Analysis,
    src_codec: Option<&str>,
    out: &Path,
    pf: &Path,
) -> (Vec<String>, String, String) {
    let c = &a.color;
    let dev = rt.intel_dev.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
    // AV1 zawsze 10-bit (jak -highbitdepth w NVENC); HEVC 10-bit tylko dla źródeł HDR/10-bit.
    let ten_bit = !c.is_sdr_8bit || opts.codec == Codec::Av1;
    let crop = a.crop.as_deref().and_then(parse_crop);
    let denoise = opts.denoise && c.is_sdr_8bit && rt.denoise != Denoise::Off;
    let cq = opts.cq() as u32;

    let mut v = sv(&["-hide_banner", "-nostats", "-progress"]);
    v.push(pf.to_string_lossy().into());

    let mut notes: Vec<String> = vec![if c.is_sdr_8bit { "SDR 8-bit".into() } else { "HDR 10-bit".into() }];
    if opts.codec == Codec::Av1 && c.is_sdr_8bit {
        notes.push("→ AV1 10-bit".into());
    }
    let decoder;
    let enc: Vec<String>;

    match opts.engine {
        Engine::Qsv => {
            v.extend(crate::hw::qsv_init_args(&dev));
            v.extend(sv(&["-filter_hw_device", "qs"]));
            match qsv_decoder(rt, src_codec) {
                Some(d) => {
                    v.extend(sv(&["-hwaccel", "qsv", "-hwaccel_device", "qs", "-hwaccel_output_format", "qsv"]));
                    v.extend(sv(&["-c:v", &d]));
                    decoder = format!("QSV ({d})");
                }
                None => decoder = format!("CPU (brak dekodera QSV dla {})", src_codec.unwrap_or("?")),
            }
            v.push("-i".into());
            v.push(a.input_file.to_string_lossy().into());

            // vpp_qsv przyjmuje klatki z karty i z pamięci (dekodowanie programowe) — jeden filtr na wszystko.
            let mut vpp: Vec<String> = vec![];
            if let Some((w, h, x, y)) = crop {
                vpp.push(format!("cw={w}:ch={h}:cx={x}:cy={y}"));
            }
            if denoise && rt.denoise == Denoise::Intel {
                vpp.push(format!("denoise={DENOISE_QSV_LEVEL}"));
                notes.push("denoise vpp_qsv".into());
            }
            vpp.push(format!("format={}", if ten_bit { "p010" } else { "nv12" }));
            v.extend(sv(&["-vf", &format!("vpp_qsv={}", vpp.join(":"))]));

            enc = match opts.codec {
                Codec::Hevc => sv(&[
                    "-c:v", "hevc_qsv", "-preset", opts.qsv_preset, "-global_quality", &cq.to_string(),
                    "-profile:v", if ten_bit { "main10" } else { "main" },
                ]),
                Codec::Av1 => sv(&[
                    "-c:v", "av1_qsv", "-preset", opts.qsv_preset, "-global_quality", &cq.to_string(),
                    "-profile:v", "main",
                ]),
            };
            notes.push(format!("ICQ {cq}"));
        }
        _ => {
            // VAAPI. Crop i odszumianie na CPU wymagają klatek w pamięci: dekodowanie nadal sprzętowe,
            // ale bez -hwaccel_output_format (ffmpeg sam pobiera klatki), potem hwupload.
            let sw_fmt = if ten_bit { "p010le" } else { "nv12" };
            let cpu_denoise = denoise && rt.denoise == Denoise::Cpu;
            let sysmem = crop.is_some() || cpu_denoise;
            v.extend(sv(&["-init_hw_device", &format!("vaapi=va:{dev}"), "-filter_hw_device", "va"]));
            v.extend(sv(&["-hwaccel", "vaapi", "-hwaccel_device", "va"]));
            if !sysmem {
                v.extend(sv(&["-hwaccel_output_format", "vaapi"]));
            }
            decoder = "VAAPI (zapasowo CPU)".into();
            v.push("-i".into());
            v.push(a.input_file.to_string_lossy().into());

            let mut chain: Vec<String> = vec![];
            if sysmem {
                if let Some(cr) = &a.crop {
                    chain.push(cr.clone());
                }
                if cpu_denoise {
                    chain.push(DENOISE_CPU_FILTER.into());
                    notes.push("denoise CPU".into());
                }
                chain.push(format!("format={sw_fmt}"));
                chain.push("hwupload".into());
            } else {
                // Klatki z dekodera VAAPI przechodzą bez kopiowania; po dekodowaniu programowym — hwupload.
                chain.push("format=nv12|p010le|vaapi".into());
                chain.push("hwupload".into());
                chain.push(format!("scale_vaapi=format={}", if ten_bit { "p010" } else { "nv12" }));
            }
            if denoise && rt.denoise == Denoise::Intel {
                chain.push(format!("denoise_vaapi=denoise={DENOISE_VAAPI_LEVEL}"));
                notes.push("denoise_vaapi".into());
            }
            v.extend(sv(&["-vf", &chain.join(",")]));

            enc = match opts.codec {
                Codec::Hevc => sv(&[
                    "-c:v", "hevc_vaapi", "-rc_mode", "CQP", "-qp", &cq.to_string(),
                    "-profile:v", if ten_bit { "main10" } else { "main" },
                ]),
                // AV1 VAAPI: base q index 0–255 — CQ przeliczone ×5 (25 → 125).
                Codec::Av1 => sv(&[
                    "-c:v", "av1_vaapi", "-rc_mode", "CQP", "-qp", &(cq * 5).min(255).to_string(),
                    "-profile:v", "main",
                ]),
            };
            notes.push(format!("CQP {cq}"));
        }
    }

    v.extend(sv(&["-an", "-sn", "-map", "0:v:0"]));
    v.extend(enc);
    if c.is_sdr_8bit {
        v.extend(sv(&["-color_primaries", &c.sdr_primaries, "-color_trc", &c.sdr_trc]));
        v.extend(sv(&["-colorspace", &c.sdr_space, "-color_range", &c.sdr_range]));
    } else {
        v.extend(sv(&["-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc"]));
    }
    v.push(out.to_string_lossy().into());
    (v, notes.join(" · "), decoder)
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
    let p = opts.paths_for(&a.input_file);
    let key = sanitize(&a.filename);
    let pf = rt.progress_dir.join(format!("{key}.ffprogress"));
    let log = rt.progress_dir.join(format!("{key}.log"));
    let _ = fs::remove_file(&pf);
    File::create(&pf)?;

    let dur_us = probe::container_duration(&a.input_file).map(|d| d * 1e6);
    panel.set_progress(&a.filename, pf.clone(), dur_us);

    let (args, note, decoder) = if opts.engine == Engine::Nvenc {
        let (args, note) = build_args_nvenc(opts, rt, a, &p.temp_video, &pf);
        (args, note, "CUDA/NVDEC".to_string())
    } else {
        let src = probe::video_codec(&a.input_file);
        build_args_intel(opts, rt, a, src.as_deref(), &p.temp_video, &pf)
    };
    let eng = opts.engine.label();
    panel.set(&a.filename, State::Nvenc, format!("{eng} {slot}/2 — {note} — dekoder: {decoder}"));

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
        panel.set(&r.name, State::NvencDone, format!("{} zakończony i zweryfikowany", opts.engine.label()));
        plog!(" -> ffmpeg [{}]: ZAKOŃCZONY + PLIK OK", r.name);
    } else {
        panel.set(
            &r.name,
            State::Error,
            format!("{} nie utworzył poprawnego pliku ({status})", opts.engine.label()),
        );
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

/// Dwa równoległe sloty kodowania; dekodowanie sprzętowe tej samej karty (NVDEC / QSV / VAAPI).
pub fn run_stage(opts: &Opts, rt: &Runtime, files: &[Analysis], panel: &Panel) {
    panel.draw(&format!("ETAP 2/3 — DUAL {}", opts.engine.label()));
    let mut running: Vec<Running> = vec![];

    for a in files {
        if proc::cancelled() {
            abort_all(&mut running, panel);
            return;
        }
        let p = opts.paths_for(&a.input_file);
        if p.output.exists() {
            panel.set(&a.filename, State::Done, "Plik wynikowy już istnieje");
            continue;
        }
        if p.temp_video.exists() {
            if existing_temp_valid(opts, &p.temp_video, &a.input_file) {
                panel.set(
                    &a.filename,
                    State::NvencDone,
                    "Kodowanie pominięte — istnieje zweryfikowany plik tymczasowy",
                );
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
