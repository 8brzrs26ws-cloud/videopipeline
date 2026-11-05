use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    Hevc,
    Av1,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Codec::Hevc => "hevc",
            Codec::Av1 => "av1",
        }
    }
}

/// Kontener pliku wynikowego: dla wejścia .mp4/.MP4 wynik też jest .mp4, w pozostałych przypadkach .mkv.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Container {
    Mkv,
    Mp4,
}

impl Container {
    pub fn from_input(input: &Path) -> Self {
        match input.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("mp4") => Container::Mp4,
            _ => Container::Mkv,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Container::Mkv => "mkv",
            Container::Mp4 => "mp4",
        }
    }
}

/// Domyślne wartości jakości (NVENC -cq, mniej = lepsza jakość i większy plik).
/// Uwaga: w skrypcie fish nazwy AV1 mówiły "cq23", ale faktyczny parametr to 25.
pub const DEFAULT_HEVC_CQ: u8 = 19;
pub const DEFAULT_AV1_CQ: u8 = 25;
/// NVENC: 0 = automat, sensowny zakres to 1..=51.
pub const CQ_MIN: u8 = 1;
pub const CQ_MAX: u8 = 51;
pub const AV1_TAG: &str = "av1_10bit_cq23";

pub const DENOISE_CUDA_FILTER: &str = "bilateral_cuda=window_size=5:sigmaS=1.5:sigmaR=12.0";
pub const DENOISE_CPU_FILTER: &str = "hqdn3d=1.2:1.0:2.0:1.5";

#[derive(Clone)]
pub struct Opts {
    pub codec: Codec,
    /// Jakość HEVC NVENC (-cq)
    pub hevc_cq: u8,
    /// Jakość AV1 NVENC (-cq)
    pub av1_cq: u8,
    pub pl_eac3: bool,
    /// -da: dodatkowo audio EN (E-AC-3 5.1) oraz napisy EN i PL
    pub extra: bool,
    pub copy: bool,
    pub denoise: bool,
    /// pomiń cropdetect
    pub no_crop: bool,
    /// pomiń ekstrakcję Dolby Vision RPU (i jej wstrzykiwanie)
    pub no_dv: bool,
    /// pomiń ekstrakcję HDR10+ (i jego wstrzykiwanie)
    pub no_hdr10plus: bool,
    pub shutdown: bool,
    pub output_dir: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Denoise {
    Off,
    Cuda,
    Cpu,
}

/// Wynik pre-flightu, wspólny dla całego uruchomienia.
pub struct Runtime {
    pub av1_tune: String,
    pub denoise: Denoise,
    pub progress_dir: PathBuf,
}

pub struct Paths {
    pub container: Container,
    /// Gotowy plik wynikowy (.mkv albo .mp4, zależnie od pliku wejściowego).
    pub output: PathBuf,
    pub temp_video: PathBuf,
    pub temp_pol_audio: PathBuf,
    pub temp_eng_audio: PathBuf,
    pub temp_rpu: PathBuf,
    pub temp_hdr10plus: PathBuf,
    pub step1: PathBuf,
    pub step2: PathBuf,
    pub analysis: PathBuf,
}

impl Opts {
    /// CQ dla aktualnie wybranego kodeka.
    pub fn cq(&self) -> u8 {
        match self.codec {
            Codec::Hevc => self.hevc_cq,
            Codec::Av1 => self.av1_cq,
        }
    }

    /// Znacznik jakości w nazwach plików — tylko gdy CQ różni się od domyślnego,
    /// dzięki czemu stare nazwy się nie zmieniają, a pliki o różnej jakości się nie mieszają.
    fn cq_tag(&self) -> String {
        let def = match self.codec {
            Codec::Hevc => DEFAULT_HEVC_CQ,
            Codec::Av1 => DEFAULT_AV1_CQ,
        };
        if self.cq() == def {
            String::new()
        } else {
            format!("_cq{}", self.cq())
        }
    }

    /// Ścieżki dla pliku wejściowego (nazwa i kontener wyniku wynikają z `input`).
    pub fn paths_for(&self, input: &Path) -> Paths {
        let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.paths(&stem, Container::from_input(input))
    }

    fn paths(&self, stem: &str, container: Container) -> Paths {
        let cqt = self.cq_tag();
        let d = &self.output_dir;
        let mut suffix = match self.codec {
            Codec::Hevc => format!("_hevc{cqt}"),
            Codec::Av1 => format!("_{AV1_TAG}{cqt}"),
        };
        if self.pl_eac3 {
            suffix.push_str("_pleac3");
        }
        if self.extra {
            suffix.push_str("_da");
        }
        let (temp_video, analysis) = match self.codec {
            Codec::Hevc => (
                d.join(format!("{stem}_encoded{cqt}.hevc")),
                d.join(format!("{stem}.analysis.json")),
            ),
            Codec::Av1 => (
                d.join(format!("{stem}_encoded_{AV1_TAG}{cqt}.mkv")),
                d.join(format!("{stem}.{AV1_TAG}.analysis.json")),
            ),
        };
        Paths {
            container,
            output: d.join(format!("{stem}{suffix}.{}", container.ext())),
            temp_video,
            temp_pol_audio: d.join(format!("{stem}_pol_192.eac3")),
            temp_eng_audio: d.join(format!("{stem}_eng.eac3")),
            temp_rpu: d.join(format!("{stem}_rpu.bin")),
            temp_hdr10plus: d.join(format!("{stem}_hdr10plus.json")),
            step1: d.join(format!("{stem}_step1{cqt}.hevc")),
            step2: d.join(format!("{stem}_step2{cqt}.hevc")),
            analysis,
        }
    }

    /// Pliki analizy pasujące do bieżącego wariantu (AV1 i HEVC mają osobne).
    pub fn list_analysis_files(&self) -> Vec<PathBuf> {
        let av1_suffix = format!(".{AV1_TAG}.analysis.json");
        let mut v: Vec<PathBuf> = fs::read_dir(&self.output_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                match self.codec {
                    Codec::Av1 => n.ends_with(&av1_suffix),
                    Codec::Hevc => n.ends_with(".analysis.json") && !n.ends_with(&av1_suffix),
                }
            })
            .collect();
        v.sort();
        v
    }
}
