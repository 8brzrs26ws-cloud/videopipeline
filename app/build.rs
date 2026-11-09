//! Windows: wkompilowanie ikony (icons/videopipeline.ico) i informacji o wersji w videopipeline.exe.
//! Ikona jest widoczna w Eksploratorze, na skrótach i — w klasycznej konsoli — na pasku zadań
//! (window.rs ustawia ją na oknie konsoli). Wymaga windres z MinGW-w64 (cel *-windows-gnu);
//! bez niego program buduje się normalnie, tylko bez ikony.
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("../icons/videopipeline.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-env-changed=WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        println!("cargo:warning=ikona .exe: obsługiwany tylko cel *-windows-gnu (windres) — pomijam");
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let ico = ico.canonicalize().unwrap_or(ico);
    let ver = env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<u32> = ver.split(['.', '-']).filter_map(|x| x.parse().ok()).chain([0, 0, 0]).take(3).collect();
    let v4 = format!("{},{},{},0", nums[0], nums[1], nums[2]);
    let rc = format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {v4}
PRODUCTVERSION {v4}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "041504B0"
    BEGIN
      VALUE "CompanyName", "VideoPipeline"
      VALUE "FileDescription", "VideoPipeline"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "videopipeline"
      VALUE "LegalCopyright", "GNU GPL v3"
      VALUE "OriginalFilename", "videopipeline.exe"
      VALUE "ProductName", "VideoPipeline"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x415, 1200
  END
END
"#,
        ico = ico.to_string_lossy().replace('\\', "/"),
    );
    let rc_path = out.join("videopipeline.rc");
    fs::write(&rc_path, rc).unwrap();
    let obj = out.join("videopipeline-res.o");

    let host_windows = env::var("HOST").is_ok_and(|h| h.contains("windows"));
    let windres = env::var("WINDRES").unwrap_or_else(|_| {
        if host_windows { "windres".into() } else { "x86_64-w64-mingw32-windres".into() }
    });
    let ok = Command::new(&windres)
        .args(["-O", "coff", "-i"])
        .arg(&rc_path)
        .arg("-o")
        .arg(&obj)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        // Obiekt z zasobami dołączony bezpośrednio do binarki (sekcja .rsrc).
        println!("cargo:rustc-link-arg-bins={}", obj.display());
    } else {
        println!("cargo:warning=nie udało się uruchomić {windres} — videopipeline.exe bez ikony");
    }
}
