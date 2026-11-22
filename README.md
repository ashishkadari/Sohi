# Sohi

An ultra-lightweight desktop voice assistant for Windows 11. Sohi stays hidden off-screen at the top-right
corner of the primary display. It slides in when you park the mouse in that corner or press a hotkey, then
listens, answers and slides away again.

> **Status:** early development (phase P0: scaffold and perf harness). It does not do anything useful yet.
> See [docs/MASTER_SPEC.md](docs/MASTER_SPEC.md) for the full design and the phase plan.

## Targets

| | Budget |
|---|---|
| Idle CPU | ≤ 60 ms per minute |
| Idle memory | ≤ 20 MB private |
| Cold start | ≤ 120 ms to armed |
| Binary | ≤ 8 MB, single `sohi.exe` |

Per-phase measurements live in [docs/perf/](docs/perf/).

## Building

Requirements:

- Windows 11 x64
- [rustup](https://rustup.rs). `rust-toolchain.toml` pins the exact toolchain, and rustup installs it on first build.
- Visual Studio 2022 Build Tools with the **Desktop development with C++** workload and a Windows 11 SDK

```powershell
cargo build --release
```

The binary is `target\release\sohi.exe`.

## Developer commands

```powershell
cargo xtask check
```
Runs fmt check, clippy with warnings as errors, all tests, and a scan for key-shaped strings. Every commit must pass it.

```powershell
cargo xtask perf --idle-secs 60 --report docs/perf/p0.md
```
Builds the release binary, runs `sohi.exe --perf` and fails if a budget is exceeded. Close any running Sohi first, because only one instance can run.

```powershell
cargo xtask bloat
```
Shows binary size per crate (needs `cargo install cargo-bloat --locked`).

## Running

```powershell
.\target\release\sohi.exe
```

| Flag | Effect |
|---|---|
| `--perf[=SECS]` | Print `perf:` timing lines on stdout, sample idle usage for SECS seconds (default 60), then exit |
| `--test-panic` | Panic right after startup to test the crash log |

Files:

- Config: `%APPDATA%\Sohi\config.toml`, written with commented defaults on first run
- Logs: `%LOCALAPPDATA%\Sohi\logs\sohi.log`, rotated at 1 MB with 3 files kept

## API keys

Not yet implemented (phase P3). Keys will be stored in Windows Credential Manager with `sohi.exe --set-key <provider>`
and will never be written to the config file or the log.
