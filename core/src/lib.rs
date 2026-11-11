//! VideoPipeline core — kompletna logika (wykrywanie GPU, analiza, NVENC/QSV/VAAPI, mux, kopiowanie, monitoring).
//! Binarka (`videopipeline`) zawiera wyłącznie interfejs: TUI albo tryb skryptowy.

/// Log pipeline'u. Zdefiniowane przed modułami, żeby było widoczne w całej bibliotece.
#[macro_export]
macro_rules! plog {
    ($($arg:tt)*) => { $crate::log::push(format!($($arg)*)) };
}

pub mod analysis;
pub mod config;
pub mod copy;
pub mod encode;
pub mod files;
pub mod hw;
pub mod inject;
pub mod intelmon;
pub mod mediainfo;
pub mod platform;
#[cfg(windows)]
pub mod winmon;
pub mod log;
pub mod model;
pub mod mux;
pub mod pipeline;
pub mod preflight;
pub mod probe;
pub mod proc;
pub mod sysmon;
