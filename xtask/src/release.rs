//! `cargo xtask release`: build Sohi and pack it as a portable zip.
//!
//! The zip holds `sohi.exe` and the README, and runs on a Windows 11 that
//! has nothing else installed: the C runtime is linked in (see
//! `.cargo/config.toml`), which is checked here.

use std::path::Path;
use std::process::Command;

use crate::{cargo, root, run_step, secrets};

/// Spec §6.
const MAX_EXE: usize = 8_000_000;
/// DLLs Windows does not come with, and features that stay out of a
/// release (spec §5.6).
const FORBIDDEN: [&str; 4] = ["VCRUNTIME140", "MSVCP140", "shell_exec", "SOHI_DEV_INJECT"];

pub fn run(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("release takes no options".to_owned());
    }
    // A folder of its own: the Sohi in daily use may run from the usual one.
    let target = root().join("target").join("dist");
    run_step(
        "cargo build --release --locked -p sohi",
        cargo()
            .args(["build", "--release", "--locked", "-p", "sohi"])
            .arg("--target-dir")
            .arg(&target)
            .env("CARGO_ENCODED_RUSTFLAGS", rustflags()),
    )?;
    let exe = target.join("release").join("sohi.exe");
    let bytes = read(&exe)?;
    check_exe(&bytes)?;

    let name = format!("sohi-{}-windows-x64", env!("CARGO_PKG_VERSION"));
    let folder = target.join(&name);
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    copy(&exe, &folder.join("sohi.exe"))?;
    copy(&root().join("README.md"), &folder.join("README.md"))?;
    let zip = target.join(format!("{name}.zip"));
    let _ = std::fs::remove_file(&zip);
    // The tar that comes with Windows writes zips; one from Git may be
    // first on the path and does not.
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    run_step(
        "tar -a -c",
        Command::new(Path::new(&windows).join("System32").join("tar.exe"))
            .args(["-a", "-c", "-f"])
            .arg(&zip)
            .arg("-C")
            .arg(&target)
            .arg(&name),
    )?;
    println!(
        "{} ({} bytes), sohi.exe {} bytes",
        zip.display(),
        read(&zip)?.len(),
        bytes.len()
    );
    Ok(())
}

/// Flags for a release: those of `.cargo/config.toml`, which they replace,
/// and no paths of this machine in panic messages.
fn rustflags() -> String {
    let mut flags = vec!["-Ctarget-feature=+crt-static".to_owned()];
    if let Ok(home) = std::env::var("USERPROFILE") {
        flags.push(format!("--remap-path-prefix={home}=~"));
    }
    flags.join("\u{1f}")
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", to.display()))
}

/// What must hold for an exe that leaves this machine.
fn check_exe(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_EXE {
        return Err(format!("sohi.exe has {} bytes", bytes.len()));
    }
    for name in FORBIDDEN {
        if bytes
            .windows(name.len())
            .any(|w| w.eq_ignore_ascii_case(name.as_bytes()))
        {
            return Err(format!("sohi.exe contains {name}"));
        }
    }
    // Paths of the build machine (a user name) do not belong in it either.
    if let Some(home) = std::env::var("USERPROFILE").ok().filter(|h| h.len() > 4)
        && bytes.windows(home.len()).any(|w| w == home.as_bytes())
    {
        return Err("sohi.exe contains the path of the build machine's user".to_owned());
    }
    let text = String::from_utf8_lossy(bytes);
    for line in text.split(|c: char| c.is_control()) {
        if let Some(what) = secrets::find(line).filter(|_| line.len() < 200) {
            return Err(format!("sohi.exe contains something like {what}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_builds_are_refused() {
        assert!(check_exe(b"MZ fine").is_ok());
        assert!(check_exe(b"MZ vcruntime140.dll").is_err());
        assert!(check_exe(b"MZ shell_exec").is_err());
        assert!(check_exe(&vec![0; MAX_EXE + 1]).is_err());
    }
}
