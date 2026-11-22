//! `cargo xtask check`: the gate every commit must pass (spec §8, §10.1).

use std::fs;

use crate::{cargo, root, run_step, secrets};

/// Files above this size are not scanned (none should be committed anyway).
const MAX_SCAN_BYTES: u64 = 1024 * 1024;

pub fn run() -> Result<(), String> {
    run_step(
        "cargo fmt --check",
        cargo().args(["fmt", "--all", "--check"]),
    )?;
    run_step(
        "cargo clippy -D warnings",
        cargo().args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]),
    )?;
    run_step("cargo test", cargo().args(["test", "--workspace"]))?;
    scan_for_secrets()?;
    eprintln!("xtask: check passed");
    Ok(())
}

/// Scans every file git would commit (tracked plus untracked, not ignored).
fn scan_for_secrets() -> Result<(), String> {
    eprintln!("xtask: secret scan");
    let output = std::process::Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root())
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err("git ls-files failed".to_owned());
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    let mut hits = Vec::new();
    for rel in listing.split('\0').filter(|s| !s.is_empty()) {
        let path = root().join(rel);
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_SCAN_BYTES {
            continue;
        }
        // Binary files are skipped: they are not valid UTF-8.
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if let Some(kind) = secrets::find(line) {
                hits.push(format!("{rel}:{}: looks like {kind}", index + 1));
            }
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        Err(format!("possible secrets found:\n  {}", hits.join("\n  ")))
    }
}
