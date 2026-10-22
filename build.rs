//! Embeds the Windows icon + version info into the executable.
//!
//! `winresource` cannot be used here: it runs `windres`, which hands the `.rc`
//! path to the C preprocessor unquoted, so any project path containing a space
//! ("My Project") fails with `cc1.exe: fatal error: Project\Engineer: No such
//! file or directory` and the executable silently ships with no icon at all.
//!
//! Instead the resource is compiled from a scratch directory whose path has no
//! spaces, using relative file names, and the resulting object file is handed
//! to the linker.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=VERSION");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Err(e) = windows_resources() { println!("cargo:warning=windows resources not embedded: {e}"); }
    }
}

/// Cargo's per-target output directory holds the generated .rc/.ico/object.
fn scratch_dir() -> Result<PathBuf, String> {
    let dir = PathBuf::from(std::env::var("OUT_DIR").map_err(|_| "OUT_DIR is unset")?);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

fn windows_resources() -> Result<(), String> {
    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR is unset")?,
    );
    let ico = manifest.join("assets").join("icon.ico");
    if !ico.is_file() {
        return Err(format!("{} is missing", ico.display()));
    }

    let version = include_str!("VERSION").trim();
    let mut nums = [0u32; 4];
    for (i, part) in version.split('.').take(4).enumerate() {
        nums[i] = part
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
    }
    let (a, b, c, d) = (nums[0], nums[1], nums[2], nums[3]);
    let product = "Download Craft";
    let description = format!("{product} - Download Manager");

    let scratch = scratch_dir()?;
    std::fs::copy(&ico, scratch.join("icon.ico"))
        .map_err(|e| format!("copying the icon: {e}"))?;

    let rc = format!(
        r#"1 ICON "icon.ico"
1 VERSIONINFO
FILEVERSION {a},{b},{c},{d}
PRODUCTVERSION {a},{b},{c},{d}
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
FILEFLAGSMASK 0x3f
FILEFLAGS 0x0
{{
BLOCK "StringFileInfo"
{{
BLOCK "000004b0"
{{
VALUE "FileDescription", "{description}"
VALUE "FileVersion", "{version}"
VALUE "ProductName", "{product}"
VALUE "ProductVersion", "{version}"
}}
}}
BLOCK "VarFileInfo" {{
VALUE "Translation", 0x0, 0x04b0
}}
}}
"#
    );
    std::fs::write(scratch.join("main.rc"), rc).map_err(|e| format!("writing the .rc: {e}"))?;

    let windres = find_windres()?;
    let output = Command::new(&windres)
        .current_dir(&scratch)
        .args(["main.rc", "-o", "main.o"])
        .output()
        .map_err(|e| format!("running {}: {e}", windres.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}",
            windres.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let object = scratch.join("main.o");
    if !object.is_file() {
        return Err("windres produced no object file".into());
    }
    println!("cargo:rustc-link-arg-bins={}", object.display());
    Ok(())
}

/// `windres` from the environment, then `PATH`, then a rustup GNU toolchain.
fn find_windres() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("WINDRES") {
        let path = Path::new(&explicit);
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
    }
    if let Ok(paths) = std::env::var("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("windres.exe");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let toolchains = PathBuf::from(home).join(".rustup").join("toolchains");
        if let Ok(entries) = std::fs::read_dir(toolchains) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("bin").join("windres.exe");
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err("windres.exe not found (set the WINDRES env var)".into())
}
