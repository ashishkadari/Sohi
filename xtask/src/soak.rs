//! `cargo xtask soak`: keep a test build running for hours, watching its
//! memory, CPU time and handles (spec §8 P8).
//!
//! First `--turns` voice turns with injected speech, one after the other,
//! against the live services; then `--hours` of being parked. The build is
//! started with a settings folder and an instance name of its own, so it
//! runs beside the Sohi in daily use. Its speech is played at -48 dB.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::{cargo, root, run_step};

const MB: f64 = 1_000_000.0;
/// Spec §8 P8: growth while parked.
const MAX_GROWTH: f64 = 1.0;
/// Spec §6: CPU time per minute parked.
const MAX_CPU_MS: f64 = 60.0;
/// How long a turn may take until Sohi is parked again.
const TURN_LIMIT: Duration = Duration::from_secs(60);
/// Parked this long before measuring begins (spec §6: 60 s).
const SETTLE: Duration = Duration::from_secs(90);
const PARKED: &str = "head: parked";

/// Never woken by the pointer, the user's hotkeys or a follow-up.
const CONFIG: &str = r#"[wake]
hotkey_voice = "Ctrl+Alt+Shift+F21"
hotkey_text = "Ctrl+Alt+Shift+F22"
corner_size_px = 1
dwell_ms = 5000
follow_up = false
[ui]
launch_peek = false
"#;

struct Options {
    turns: u32,
    hours: f64,
    wavs: Vec<PathBuf>,
    report: Option<PathBuf>,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        turns: 200,
        hours: 8.0,
        wavs: Vec::new(),
        report: None,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--turns" => opts.turns = value()?.parse().map_err(|e| format!("--turns: {e}"))?,
            "--hours" => opts.hours = value()?.parse().map_err(|e| format!("--hours: {e}"))?,
            "--wav" => opts.wavs.push(PathBuf::from(value()?)),
            "--report" => opts.report = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown soak option {other}")),
        }
    }
    if opts.turns > 0 && opts.wavs.is_empty() {
        return Err("turns need at least one --wav with a spoken question".to_owned());
    }
    if !(0.0..=48.0).contains(&opts.hours) {
        return Err("--hours must be 0 to 48".to_owned());
    }
    Ok(opts)
}

/// What the process holds at one moment.
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    private_mb: f64,
    working_set_mb: f64,
    cpu_ms: f64,
    handles: f64,
    threads: f64,
    gdi: f64,
    user: f64,
}

impl Sample {
    /// Reads the line [`SAMPLER`] prints.
    fn parse(line: &str) -> Option<Self> {
        let v: Vec<f64> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let [private, working_set, cpu_ms, handles, threads, gdi, user] = v[..] else {
            return None;
        };
        Some(Self {
            private_mb: private / MB,
            working_set_mb: working_set / MB,
            cpu_ms,
            handles,
            threads,
            gdi,
            user,
        })
    }

    fn row(&self, label: &str) -> String {
        format!(
            "| {label} | {:.2} | {:.2} | {:.0} | {:.0} | {:.0} | {:.0} |\n",
            self.private_mb, self.working_set_mb, self.handles, self.threads, self.gdi, self.user
        )
    }
}

/// `xtask` has no FFI, so PowerShell asks Windows. GDI and USER objects are
/// not among what `Get-Process` knows.
const SAMPLER: &str = r#"$ErrorActionPreference='Stop';
$t = Add-Type -PassThru -Name G -Namespace S -MemberDefinition '[DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr h, uint f);';
$p = Get-Process -Id PID;
[string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0} {1} {2} {3} {4} {5} {6}', $p.PrivateMemorySize64, $p.WorkingSet64, $p.TotalProcessorTime.TotalMilliseconds, $p.HandleCount, $p.Threads.Count, $t::GetGuiResources($p.Handle,0), $t::GetGuiResources($p.Handle,1))"#;

/// The build under test.
struct Sohi {
    child: Child,
    exe: PathBuf,
    dir: PathBuf,
    name: String,
}

impl Sohi {
    fn start(exe: &Path) -> Result<Self, String> {
        let name = format!("Soak{}", std::process::id());
        let dir = root().join("target").join("soak");
        let _ = std::fs::remove_dir_all(dir.join("local"));
        let settings = dir.join("roaming").join("Sohi");
        std::fs::create_dir_all(&settings).map_err(|e| format!("{}: {e}", settings.display()))?;
        std::fs::write(settings.join("config.toml"), CONFIG).map_err(|e| e.to_string())?;
        // Cargo may replace its own copy while this runs.
        let copy = dir.join("sohi.exe");
        std::fs::copy(exe, &copy).map_err(|e| format!("{}: {e}", copy.display()))?;
        let child = command(&copy, &dir, &name)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", copy.display()))?;
        sleep(Duration::from_secs(3));
        Ok(Self {
            child,
            exe: copy,
            dir,
            name,
        })
    }

    fn check_alive(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => Err(format!("Sohi ended ({status})")),
            Err(e) => Err(e.to_string()),
        }
    }

    fn sample(&mut self) -> Result<Sample, String> {
        self.check_alive()?;
        let script = SAMPLER.replace("PID", &self.child.id().to_string());
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| format!("cannot start PowerShell: {e}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        Sample::parse(text.trim()).ok_or_else(|| {
            let why = String::from_utf8_lossy(&output.stderr);
            format!("cannot read the process: {text} {why}")
        })
    }

    /// The log so far, the older files first.
    fn log(&self) -> String {
        let logs = self.dir.join("local").join("Sohi").join("logs");
        ["sohi.2.log", "sohi.1.log", "sohi.log"]
            .iter()
            .filter_map(|name| std::fs::read_to_string(logs.join(name)).ok())
            .collect()
    }

    /// One voice turn: what the log says of it.
    fn turn(&mut self, wav: &Path) -> Result<String, String> {
        let before = self.log();
        let mark = before.lines().last().unwrap_or_default().to_owned();
        // The question is read from a fixed path, so each can differ.
        std::fs::copy(wav, self.dir.join("question.wav")).map_err(|e| e.to_string())?;
        command(&self.exe, &self.dir, &self.name)
            .status()
            .map_err(|e| format!("cannot wake Sohi: {e}"))?;
        let deadline = Instant::now() + TURN_LIMIT;
        loop {
            sleep(Duration::from_millis(250));
            self.check_alive()?;
            let log = self.log();
            let new = match log.find(&mark) {
                Some(at) => &log[at + mark.len()..],
                None => &log[..],
            };
            if new.contains(PARKED) {
                return Ok(new.to_owned());
            }
            if Instant::now() > deadline {
                return Err("a turn never ended".to_owned());
            }
        }
    }
}

impl Drop for Sohi {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command(exe: &Path, dir: &Path, name: &str) -> Command {
    let mut command = Command::new(exe);
    command
        .env("APPDATA", dir.join("roaming"))
        .env("LOCALAPPDATA", dir.join("local"))
        .env("SOHI_DEV_INSTANCE", name)
        .env("SOHI_DEV_INJECT", dir.join("question.wav"))
        .env("SOHI_DEV_QUIET", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[derive(Debug, Default)]
struct Turns {
    heard: u32,
    failed: Vec<String>,
    waits: Vec<u32>,
    samples: Vec<(u32, Sample)>,
}

fn run_turns(sohi: &mut Sohi, opts: &Options) -> Result<Turns, String> {
    let mut turns = Turns::default();
    for n in 1..=opts.turns {
        let wav = &opts.wavs[(n as usize - 1) % opts.wavs.len()];
        let log = sohi.turn(wav)?;
        if let Some(ms) = first_audio_ms(&log) {
            turns.heard += 1;
            turns.waits.push(ms);
        } else {
            let why = log
                .lines()
                .find(|l| l.contains("WARN") || l.contains("ERROR"));
            turns
                .failed
                .push(format!("turn {n}: {}", why.unwrap_or("no reply heard")));
        }
        if n % 20 == 0 || n == 10 {
            let sample = sohi.sample()?;
            eprintln!("xtask: turn {n}: {sample:?}");
            turns.samples.push((n, sample));
        }
        // The services allow only so many requests a minute.
        sleep(Duration::from_secs(3));
    }
    Ok(turns)
}

/// `first audio 1234 ms after the utterance ended` in a turn's log.
fn first_audio_ms(log: &str) -> Option<u32> {
    let (_, rest) = log.split_once("first audio ")?;
    rest.split_once(" ms")?.0.parse().ok()
}

#[derive(Debug, Default)]
struct Parked {
    samples: Vec<(Duration, Sample)>,
}

impl Parked {
    fn growth_mb(&self) -> f64 {
        let first = self.samples.first().map_or(0.0, |(_, s)| s.private_mb);
        let last = self.samples.last().map_or(0.0, |(_, s)| s.private_mb);
        last - first
    }

    /// CPU time per minute, over the whole time.
    fn cpu_ms_per_min(&self) -> f64 {
        let (Some((from, a)), Some((to, b))) = (self.samples.first(), self.samples.last()) else {
            return 0.0;
        };
        let minutes = to.saturating_sub(*from).as_secs_f64() / 60.0;
        if minutes > 0.0 {
            (b.cpu_ms - a.cpu_ms) / minutes
        } else {
            0.0
        }
    }
}

fn run_parked(sohi: &mut Sohi, hours: f64) -> Result<Parked, String> {
    sleep(SETTLE);
    let started = Instant::now();
    let length = Duration::from_secs_f64(hours * 3600.0);
    let every = Duration::from_secs(600).min(length / 6);
    let mut parked = Parked::default();
    loop {
        let sample = sohi.sample()?;
        eprintln!("xtask: parked {:?}: {sample:?}", started.elapsed());
        parked.samples.push((started.elapsed(), sample));
        let left = length.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Ok(parked);
        }
        sleep(every.min(left));
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let opts = parse_options(args)?;
    // A folder of its own: the Sohi in daily use may run from the usual one.
    let target = root().join("target").join("soak").join("build");
    run_step(
        "cargo build --release -p sohi --features dev-inject",
        cargo()
            .args(["build", "--release", "-p", "sohi"])
            .args(["--features", "dev-inject", "--target-dir"])
            .arg(&target),
    )?;
    let mut sohi = Sohi::start(&target.join("release").join("sohi.exe"))?;
    let turns = run_turns(&mut sohi, &opts)?;
    let parked = run_parked(&mut sohi, opts.hours)?;
    let (report, failed) = report(&opts, &turns, &parked);
    println!("{report}");
    if let Some(path) = &opts.report {
        std::fs::write(path, &report).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("soak failed: {}", failed.join(", ")))
    }
}

/// The report, and what failed.
fn report(opts: &Options, turns: &Turns, parked: &Parked) -> (String, Vec<&'static str>) {
    let mut failed = Vec::new();
    let mut out = String::new();
    let head = "| | private MB | working set MB | handles | threads | GDI | USER |\n\
                |---|---|---|---|---|---|---|\n";
    let _ = writeln!(out, "Turns: {} of {} heard.", turns.heard, opts.turns);
    let mut waits = turns.waits.clone();
    waits.sort_unstable();
    if !waits.is_empty() {
        let _ = writeln!(
            out,
            "First audio: p50 {} ms, p95 {} ms.",
            waits[(waits.len() - 1) / 2],
            waits[(waits.len() - 1) * 95 / 100]
        );
    }
    for line in &turns.failed {
        let _ = writeln!(out, "- {line}");
    }
    let _ = write!(out, "\n{head}");
    for (n, sample) in &turns.samples {
        out.push_str(&sample.row(&format!("after {n} turns")));
    }
    // Every hour, and the end.
    let mut next = Duration::ZERO;
    for (i, (at, sample)) in parked.samples.iter().enumerate() {
        if *at >= next || i + 1 == parked.samples.len() {
            out.push_str(&sample.row(&format!("parked {:.1} h", at.as_secs_f64() / 3600.0)));
            next = *at + Duration::from_secs(3500);
        }
    }
    let _ = writeln!(
        out,
        "\nParked: private bytes grew {:+.2} MB (at most {MAX_GROWTH}); {:.1} ms CPU per minute (at most {MAX_CPU_MS}).",
        parked.growth_mb(),
        parked.cpu_ms_per_min()
    );
    if parked.growth_mb() > MAX_GROWTH {
        failed.push("memory grew while parked");
    }
    if parked.cpu_ms_per_min() > MAX_CPU_MS {
        failed.push("too much CPU while parked");
    }
    // A service may fail now and then; Sohi may not.
    if turns.heard * 20 < opts.turns * 19 {
        failed.push("more than 1 in 20 turns failed");
    }
    (out, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_and_waits_are_read() {
        let sample = Sample::parse("4700000 24600000 31.25 210 7 12 9").unwrap();
        assert!((sample.private_mb - 4.7).abs() < 1e-9);
        assert!((sample.user - 9.0).abs() < 1e-9);
        assert!(Sample::parse("1 2").is_none());
        let log = "INFO sohi::speak: turn 1: first audio 1234 ms after the utterance ended";
        assert_eq!(first_audio_ms(log), Some(1234));
        assert_eq!(first_audio_ms("nothing"), None);
    }

    #[test]
    fn growth_and_cpu_are_judged() {
        let at = |secs, private_mb, cpu_ms| {
            let sample = Sample {
                private_mb,
                cpu_ms,
                ..Sample::default()
            };
            (Duration::from_secs(secs), sample)
        };
        let parked = Parked {
            samples: vec![at(0, 5.0, 100.0), at(120, 6.5, 300.0)],
        };
        assert!((parked.growth_mb() - 1.5).abs() < 1e-9);
        assert!((parked.cpu_ms_per_min() - 100.0).abs() < 1e-9);
        let opts = parse_options(&["--turns".to_owned(), "0".to_owned()]).unwrap();
        let (_, failed) = report(&opts, &Turns::default(), &parked);
        assert_eq!(failed.len(), 2);
    }

    #[test]
    fn turns_need_a_question() {
        assert!(parse_options(&[]).is_err());
    }
}
