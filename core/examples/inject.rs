//! Narzędzie testowe: `inject <fps num/den> [rpu.bin|-] [hdr10plus.json|-] < in.hevc > out.mkv`
//! (porównanie wstrzykiwania w locie z dovi_tool / hdr10plus_tool + mkvmerge).
use std::{io, path::Path};
use videopipeline_core::inject;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (num, den) = a[1].split_once('/').unwrap_or((&a[1], "1"));
    let opt = |i: usize| a.get(i).filter(|s| *s != "-").map(|s| Path::new(s.as_str()).to_path_buf());
    let meta = inject::Metadata::load(opt(2).as_deref(), opt(3).as_deref(), None)?;
    let info = inject::StreamInfo { fps: (num.parse()?, den.parse()?), hlg: false };
    let stats = inject::run(io::stdin().lock(), io::BufWriter::new(io::stdout().lock()), &meta, info)?;
    eprintln!("{stats:?}");
    for w in stats.warnings() {
        eprintln!("UWAGA: {w}");
    }
    Ok(())
}
