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

    // zig (the Linux release packages) lacks libgcc's CPU feature table,
    // which UnRAR's feature checks read: link an empty one.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") && built_with_zig() {
        println!("cargo:rerun-if-changed=build/cpu_model.c");
        cc::Build::new().file("build/cpu_model.c").compile("dc_cpu_model");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Err(e) = windows_resources() { println!("cargo:warning=windows resources not embedded: {e}"); }
        // MinGW: the RAR reader is C++ and asks for `-lstdc++ -lpthread`,
        // which would pick the DLL import libraries: the exe would then need
        // libstdc++-6.dll, and the DLL pthread collides with the static one
        // Rust's std links. A folder holding only the static archives is
        // searched first, so both are linked into the executable.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            if let Err(e) = static_cxx_runtime() {
                println!("cargo:warning=static C++ runtime not set up: {e}");
            }
        }
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

/// Copy MinGW's static libstdc++ and winpthread archives into a folder of
/// their own and put it on the linker's search path.
fn static_cxx_runtime() -> Result<(), String> {
    let dir = scratch_dir()?.join("static-cxx");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let gcc = std::env::var("CC").unwrap_or_else(|_| "x86_64-w64-mingw32-gcc".into());
    for lib in ["libstdc++.a", "libpthread.a"] {
        let out = Command::new(&gcc)
            .arg(format!("-print-file-name={lib}"))
            .output()
            .or_else(|_| Command::new("gcc").arg(format!("-print-file-name={lib}")).output())
            .map_err(|e| format!("gcc: {e}"))?;
        let found = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if !found.is_absolute() || !found.is_file() {
            return Err(format!("{lib} not found"));
        }
        std::fs::copy(&found, dir.join(lib)).map_err(|e| format!("{lib}: {e}"))?;
    }
    println!("cargo:rustc-link-search=native={}", dir.display());
    Ok(())
}

/// cargo-zigbuild points the C compiler variables at its zig wrappers.
fn built_with_zig() -> bool {
    std::env::vars().any(|(k, v)| (k == "CC" || k == "TARGET_CC" || k.starts_with("CC_")) && v.contains("zig"))
}
