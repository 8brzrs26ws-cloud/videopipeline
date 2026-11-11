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

    pub fn label(self) -> &'static str {
        match self {
            Codec::Hevc => "HEVC",
            Codec::Av1 => "AV1",
        }
    }
}

/// Sprzętowy silnik kodujący. NVIDIA: NVENC (+ dekodowanie NVDEC/CUDA).
/// Intel: QSV (oneVPL, + dekodowanie QSV) — domyślny; VAAPI — zapasowy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Nvenc,
    Qsv,
    Vaapi,
}

impl Engine {
    pub const ALL: [Engine; 3] = [Engine::Nvenc, Engine::Qsv, Engine::Vaapi];

    pub fn label(self) -> &'static str {
        match self {
            Engine::Nvenc => "NVENC",
            Engine::Qsv => "QSV",
            Engine::Vaapi => "VAAPI",
        }
    }

    pub fn vendor(self) -> &'static str {
        match self {
            Engine::Nvenc => "NVIDIA",
            Engine::Qsv | Engine::Vaapi => "Intel",
        }
    }

    pub fn is_intel(self) -> bool {
        self != Engine::Nvenc
    }

    /// Nazwa enkodera w ffmpeg.
    pub fn encoder(self, c: Codec) -> &'static str {
        match (self, c) {
            (Engine::Nvenc, Codec::Hevc) => "hevc_nvenc",
            (Engine::Nvenc, Codec::Av1) => "av1_nvenc",
            (Engine::Qsv, Codec::Hevc) => "hevc_qsv",
            (Engine::Qsv, Codec::Av1) => "av1_qsv",
            (Engine::Vaapi, Codec::Hevc) => "hevc_vaapi",
            (Engine::Vaapi, Codec::Av1) => "av1_vaapi",
        }
    }

    pub fn from_name(s: &str) -> Option<Engine> {
        match s.to_ascii_lowercase().as_str() {
            "nvenc" | "nvidia" => Some(Engine::Nvenc),
            "qsv" => Some(Engine::Qsv),
            "vaapi" => Some(Engine::Vaapi),
            _ => None,
        }
    }

    /// Znacznik w nazwach plików — NVENC bez znacznika (zgodność ze starszymi wynikami).
    fn file_tag(self) -> &'static str {
        match self {
            Engine::Nvenc => "",
            Engine::Qsv => "_qsv",
            Engine::Vaapi => "_vaapi",
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

/// Wybór kontenera w opcjach: jak źródło (MP4 → MP4, reszta → MKV) albo zawsze MKV / MP4.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContainerChoice {
    Auto,
    Mkv,
    Mp4,
}

impl ContainerChoice {
    pub const ALL: [ContainerChoice; 3] = [ContainerChoice::Auto, ContainerChoice::Mkv, ContainerChoice::Mp4];

    pub fn resolve(self, input: &Path) -> Container {
        match self {
            ContainerChoice::Auto => Container::from_input(input),
            ContainerChoice::Mkv => Container::Mkv,
            ContainerChoice::Mp4 => Container::Mp4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContainerChoice::Auto => "jak źródło",
            ContainerChoice::Mkv => "MKV",
            ContainerChoice::Mp4 => "MP4",
        }
    }

    pub fn from_name(s: &str) -> Option<ContainerChoice> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(ContainerChoice::Auto),
            "mkv" | "matroska" => Some(ContainerChoice::Mkv),
            "mp4" => Some(ContainerChoice::Mp4),
            _ => None,
        }
    }
}

/// Domyślne wartości jakości (mniej = lepsza jakość i większy plik). Ta sama liczba trafia do
/// NVENC (-cq), QSV (ICQ -global_quality) i VAAPI (CQP -qp; AV1 w skali 0–255, przeliczane ×5).
/// Uwaga: w skrypcie fish nazwy AV1 mówiły "cq23", ale faktyczny parametr to 25.
pub const DEFAULT_HEVC_CQ: u8 = 19;
pub const DEFAULT_AV1_CQ: u8 = 25;
/// NVENC: 0 = automat, sensowny zakres to 1..=51.
pub const CQ_MIN: u8 = 1;
pub const CQ_MAX: u8 = 51;
pub const AV1_TAG: &str = "av1_10bit_cq23";

/// Presety enkodera Intel QSV (od najszybszego do najwolniejszego, najlepszego).
/// Na zintegrowanej grafice veryslow bywa kilka razy wolniejszy od slow przy niewielkim zysku jakości.
pub const QSV_PRESETS: [&str; 7] = ["veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow"];
pub const DEFAULT_QSV_PRESET: &str = "slow";

pub const DENOISE_CUDA_FILTER: &str = "bilateral_cuda=window_size=5:sigmaS=1.5:sigmaR=12.0";
pub const DENOISE_CPU_FILTER: &str = "hqdn3d=1.2:1.0:2.0:1.5";
/// Łagodne odszumianie na karcie Intel (vpp_qsv: 0–100, denoise_vaapi: 0–64).
pub const DENOISE_QSV_LEVEL: u32 = 16;
pub const DENOISE_VAAPI_LEVEL: u32 = 10;

#[derive(Clone)]
pub struct Opts {
    pub codec: Codec,
    /// Silnik kodujący (NVENC / QSV / VAAPI)
    pub engine: Engine,
    /// Jakość HEVC (CQ)
    pub hevc_cq: u8,
    /// Jakość AV1 (CQ)
    pub av1_cq: u8,
    /// Preset QSV (jeden z QSV_PRESETS); NVENC ma stały p7, VAAPI nie ma presetów.
    pub qsv_preset: &'static str,
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
    /// Kontener pliku wynikowego (auto = jak źródło).
    pub container: ContainerChoice,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Denoise {
    Off,
    Cuda,
    /// Filtr sprzętowy karty Intel (vpp_qsv denoise / denoise_vaapi)
    Intel,
    Cpu,
}

/// Wynik pre-flightu, wspólny dla całego uruchomienia.
pub struct Runtime {
    pub av1_tune: String,
    pub denoise: Denoise,
    pub progress_dir: PathBuf,
    /// Węzeł renderD karty Intel (QSV/VAAPI).
    pub intel_dev: Option<PathBuf>,
    /// Dekodery QSV dostępne w ffmpeg (np. "hevc_qsv").
    pub qsv_decoders: Vec<String>,
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

impl Paths {
    /// Zakodowane wideo w którejkolwiek postaci (po NVENC/QSV albo już z HDR10+ / Dolby Vision) —
    /// pliki pośrednie są usuwane po każdym udanym kroku, więc może zostać tylko ostatni.
    pub fn has_encoded_video(&self) -> bool {
        self.temp_video.exists() || self.step1.exists() || self.step2.exists()
    }
}

impl Opts {
    /// Np. "HEVC NVENC", "AV1 QSV".
    pub fn codec_label(&self) -> String {
        format!("{} {}", self.codec.label(), self.engine.label())
    }

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
    /// Wideo kodowane od razu do pliku wynikowego (bez plików tymczasowych .hevc i muksowania):
    /// gdy nie ma czego wstrzykiwać — brak Dolby Vision i HDR10+ albo ich analiza pominięta, oraz AV1
    /// (tam DV/HDR10+ i tak nie są przenoszone).
    pub fn direct_encode(&self, has_dovi: bool, has_hdr10plus: bool) -> bool {
        self.codec == Codec::Av1 || !((has_dovi && !self.no_dv) || (has_hdr10plus && !self.no_hdr10plus))
    }

    pub fn paths_for(&self, input: &Path) -> Paths {
        let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        self.paths(&stem, self.container.resolve(input))
    }

    fn paths(&self, stem: &str, container: Container) -> Paths {
        let cqt = format!("{}{}", self.engine.file_tag(), self.cq_tag());
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
