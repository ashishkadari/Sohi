//! `cargo xtask perf`: run the release build in `--perf` mode and check the
//! spec §6 budgets that apply so far.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::{cargo, root, run_step};

/// Decimal megabyte, the stricter reading of "MB" in the spec.
const MB: f64 = 1_000_000.0;

struct Options {
    idle_secs: u64,
    report: Option<PathBuf>,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        idle_secs: 60,
        report: None,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--idle-secs" => {
                opts.idle_secs = value()?.parse().map_err(|e| format!("--idle-secs: {e}"))?;
            }
            "--report" => opts.report = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown perf option {other}")),
        }
    }
    Ok(opts)
}

/// One budget row: metric, measured value, limit, unit.
struct Row {
    metric: &'static str,
    value: f64,
    limit: f64,
    unit: &'static str,
}

impl Row {
    fn passed(&self) -> bool {
        self.value <= self.limit
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let opts = parse_options(args)?;
    run_step(
        "cargo build --release -p sohi",
        cargo().args(["build", "--release", "-p", "sohi"]),
    )?;
    let exe = root().join("target").join("release").join("sohi.exe");
    let size = std::fs::metadata(&exe)
        .map_err(|e| format!("{}: {e}", exe.display()))?
        .len();
    eprintln!(
        "xtask: running sohi --perf={} (keep the mouse away from the corner)",
        opts.idle_secs
    );
    let metrics = run_perf_mode(&exe, opts.idle_secs)?;
    let rows = budget_rows(&metrics, size)?;
    let table = render_table(&rows);
    println!("{table}");
    if let Some(path) = &opts.report {
        std::fs::write(path, render_report(&table, opts.idle_secs))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!("xtask: wrote {}", path.display());
    }
    let failed: Vec<_> = rows
        .iter()
        .filter(|r| !r.passed())
        .map(|r| r.metric)
        .collect();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("budget exceeded: {}", failed.join(", ")))
    }
}

/// Runs `exe --perf=<idle>` and collects its `perf: key=value` lines.
fn run_perf_mode(exe: &std::path::Path, idle_secs: u64) -> Result<BTreeMap<String, f64>, String> {
    let mut child = Command::new(exe)
        .arg(format!("--perf={idle_secs}"))
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(idle_secs + 30);
    let mut metrics = BTreeMap::new();
    while !metrics.contains_key("done") {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => parse_line(&line, &mut metrics),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                return Err("sohi --perf timed out".to_owned());
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !metrics.contains_key("armed_ms") {
        return Err(format!(
            "no perf output (exit {status}); is another Sohi instance already running?"
        ));
    }
    Ok(metrics)
}

fn parse_line(line: &str, metrics: &mut BTreeMap<String, f64>) {
    let Some(rest) = line.strip_prefix("perf: ") else {
        return;
    };
    if let Some((key, value)) = rest.split_once('=')
        && let Ok(value) = value.trim().parse::<f64>()
    {
        metrics.insert(key.to_owned(), value);
    }
}

#[allow(clippy::cast_precision_loss)] // sizes are far below 2^52 bytes.
fn budget_rows(m: &BTreeMap<String, f64>, size: u64) -> Result<Vec<Row>, String> {
    let get = |k: &str| {
        m.get(k)
            .copied()
            .ok_or_else(|| format!("missing perf metric {k}"))
    };
    let wall_ms = get("idle_wall_ms")?.max(1.0);
    let row = |metric, value, limit, unit| Row {
        metric,
        value,
        limit,
        unit,
    };
    Ok(vec![
        row("cold start to armed", get("armed_ms")?, 120.0, "ms"),
        row(
            "pre-first-wake private",
            get("armed_private_bytes")? / MB,
            10.0,
            "MB",
        ),
        row("idle private", get("idle_private_bytes")? / MB, 20.0, "MB"),
        row(
            "idle working set",
            get("idle_working_set")? / MB,
            25.0,
            "MB",
        ),
        row(
            "idle CPU per 60 s",
            get("idle_cpu_ms")? * 60_000.0 / wall_ms,
            60.0,
            "ms",
        ),
        row("binary size", size as f64 / MB, 8.0, "MB"),
    ])
}

fn render_table(rows: &[Row]) -> String {
    let mut out = String::from("| Metric | Measured | Budget | Result |\n|---|---|---|---|\n");
    for r in rows {
        let verdict = if r.passed() { "pass" } else { "FAIL" };
        let _ = writeln!(
            out,
            "| {} | {:.2} {} | ≤ {} {} | {verdict} |",
            r.metric, r.value, r.unit, r.limit, r.unit
        );
    }
    out
}

fn render_report(table: &str, idle_secs: u64) -> String {
    let machine = machine_description();
    format!(
        "# Perf report\n\n\
         Generated by `cargo xtask perf --idle-secs {idle_secs}` on the release build.\n\n\
         **Machine:** {machine}\n\n{table}"
    )
}

/// CPU, RAM and OS build, read through PowerShell (no FFI in xtask).
fn machine_description() -> String {
    let script = "$c=(Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim();\
                  $r=[math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory/1GB);\
                  $o=Get-CimInstance Win32_OperatingSystem;\
                  \"$c, $r GB RAM, $($o.Caption) build $($o.BuildNumber)\"";
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_perf_lines_and_ignores_others() {
        let mut m = BTreeMap::new();
        for line in ["perf: armed_ms=17.4", "noise", "perf: bad", "perf: done=1"] {
            parse_line(line, &mut m);
        }
        assert_eq!(m.len(), 2);
        assert!((m["armed_ms"] - 17.4).abs() < f64::EPSILON);
    }

    #[test]
    fn cpu_budget_is_scaled_to_60_seconds() {
        let m: BTreeMap<String, f64> = [
            ("armed_ms", 10.0),
            ("armed_private_bytes", 1e6),
            ("idle_private_bytes", 1e6),
            ("idle_working_set", 1e6),
            ("idle_wall_ms", 6_000.0),
            ("idle_cpu_ms", 7.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let rows = budget_rows(&m, 1_000_000).unwrap();
        let cpu = rows
            .iter()
            .find(|r| r.metric == "idle CPU per 60 s")
            .unwrap();
        assert!((cpu.value - 70.0).abs() < 1e-9);
        assert!(!cpu.passed());
    }
}
