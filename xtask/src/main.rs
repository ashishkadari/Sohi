//! Developer task runner for the Sohi workspace (`cargo xtask <command>`).
//!
//! Commands:
//! - `check`: fmt --check, clippy -D warnings, tests, and a scan for
//!   key-shaped strings in files git would commit.
//! - `perf [--idle-secs N] [--report PATH]`: release build, run
//!   `sohi.exe --perf`, check the spec §6 budgets, optionally write a report.
//! - `bloat`: per-crate size breakdown of the release binary (`cargo bloat`).
//! - `soak --wav FILE... [--turns N] [--hours H] [--report PATH]`: voice turns
//!   with injected speech, then hours of being parked (spec §8 P8).
//! - `snap [--out DIR]`: render every visual state at 100/150/200 % to PNGs.

#![forbid(unsafe_code)]

mod check;
mod perf;
mod png;
mod secrets;
mod snap;
mod soak;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <check | perf [--idle-secs N] [--report PATH] | bloat | soak --wav FILE... [--turns N] [--hours H] [--report PATH] | snap [--out DIR]>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check") => check::run(),
        Some("perf") => perf::run(&args[1..]),
        Some("bloat") => bloat(),
        Some("soak") => soak::run(&args[1..]),
        Some("snap") => snap::run(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("xtask: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (the parent of the `xtask` crate).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// A `cargo` command rooted at the workspace, using the invoking cargo.
fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root());
    cmd
}

/// Runs `cmd`, failing with `label` if it cannot start or exits non-zero.
fn run_step(label: &str, cmd: &mut Command) -> Result<(), String> {
    eprintln!("xtask: {label}");
    let status = cmd
        .status()
        .map_err(|e| format!("{label}: cannot start: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} failed ({status})"))
    }
}

fn bloat() -> Result<(), String> {
    let installed = cargo()
        .args(["bloat", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !installed {
        return Err("cargo-bloat is not installed: cargo install cargo-bloat --locked".to_owned());
    }
    run_step(
        "cargo bloat --release --crates",
        cargo().args(["bloat", "--release", "--crates", "-p", "sohi", "-n", "20"]),
    )
}
