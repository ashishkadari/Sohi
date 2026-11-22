//! Rolling file log sink and panic hook (spec §2.2, §9.5).
//!
//! Lines go to `sohi.log`; when it would pass 1 MiB it is rotated to
//! `sohi.1.log`, which moves to `sohi.2.log`, so at most three files exist.
//! Writes are unbuffered (one `write` per line) so nothing is lost when the
//! process is killed or aborts.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const FILE_NAMES: [&str; 3] = ["sohi.log", "sohi.1.log", "sohi.2.log"];

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

struct FileLogger {
    sink: Mutex<Sink>,
}

struct Sink {
    dir: PathBuf,
    file: Option<File>,
    len: u64,
}

impl Sink {
    fn open(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let file = open_append(&dir.join(FILE_NAMES[0]))?;
        let len = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            file: Some(file),
            len,
        })
    }

    fn write_line(&mut self, line: &str) {
        let bytes = line.len() as u64;
        if self.len > 0 && self.len + bytes > MAX_FILE_BYTES {
            self.rotate();
        }
        if let Some(file) = self.file.as_mut()
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.len += bytes;
        }
    }

    fn rotate(&mut self) {
        // Close before renaming: Windows refuses to rename an open file.
        self.file = None;
        let path = |i: usize| self.dir.join(FILE_NAMES[i]);
        let _ = fs::remove_file(path(2));
        let _ = fs::rename(path(1), path(2));
        let _ = fs::rename(path(0), path(1));
        self.file = open_append(&path(0)).ok();
        self.len = 0;
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}\n",
            format_utc(now_unix_millis()),
            record.level(),
            record.target(),
            record.args()
        );
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_line(&line);
    }

    fn flush(&self) {}
}

/// Default log directory: `%LOCALAPPDATA%\Sohi\logs`.
pub fn default_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("Sohi")
        .join("logs")
}

/// Installs the file logger as the global `log` sink.
///
/// # Errors
/// Fails if the directory or file cannot be opened, or a logger is already set.
pub fn init(dir: &Path, level: LevelFilter) -> io::Result<()> {
    let sink = Sink::open(dir)?;
    let logger = LOGGER.get_or_init(|| FileLogger {
        sink: Mutex::new(sink),
    });
    log::set_logger(logger).map_err(io::Error::other)?;
    log::set_max_level(level);
    Ok(())
}

/// Installs a panic hook that logs the message and a backtrace, then aborts.
///
/// Aborting (instead of unwinding) matches the release profile's
/// `panic = "abort"` and gives a non-zero exit code in every build.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!("panic: {info}\nbacktrace:\n{backtrace}");
        std::process::abort();
    }));
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Formats milliseconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
fn format_utc(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let (year, month, day) = civil_from_days(secs / 86_400);
    let rem = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        unix_ms % 1000
    )
}

/// Converts days since 1970-01-01 to a (year, month, day) civil date.
///
/// Howard Hinnant's `civil_from_days`, restricted to dates after the epoch.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn formats_leap_day_and_millis() {
        // 2024-02-29T12:34:56.789Z
        assert_eq!(format_utc(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
    }

    #[test]
    fn formats_year_end() {
        // 2026-12-31T23:59:59.999Z
        assert_eq!(format_utc(1_798_761_599_999), "2026-12-31T23:59:59.999Z");
    }

    #[test]
    fn rotates_at_limit_and_keeps_three_files() {
        let dir = std::env::temp_dir().join(format!("sohi-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut sink = Sink::open(&dir).unwrap();
        let line = format!("{}\n", "x".repeat(1023));
        // Four files' worth of lines: forces three rotations.
        for _ in 0..(4 * 1024) {
            sink.write_line(&line);
        }
        drop(sink);
        let mut names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["sohi.1.log", "sohi.2.log", "sohi.log"]);
        for name in &names {
            assert!(fs::metadata(dir.join(name)).unwrap().len() <= MAX_FILE_BYTES);
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
