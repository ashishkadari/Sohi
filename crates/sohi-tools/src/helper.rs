//! The shell, from a short-lived process.
//!
//! Opening something and listing the apps folder each load about 36 system
//! DLLs, which then stay in the process for good (some 20 MB of working
//! set). So Sohi starts itself with [`OPEN`] or [`LIST`] for that: the
//! DLLs go when the helper ends.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Argument: open the target that follows.
pub const OPEN: &str = "--open";
/// Argument: print the apps folder, one app a line: name, tab, target.
pub const LIST: &str = "--list-apps";

/// `CREATE_NO_WINDOW`.
const NO_WINDOW: u32 = 0x0800_0000;
/// Opening can wait for the user (an administrator prompt). It is taken to
/// have worked after this long.
const OPEN_WAIT: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(5);

/// In the helper: opens `target`. Returns whether that worked.
#[must_use]
pub fn run_open(target: &str) -> bool {
    sohi_win::shell::open_path(Path::new(target)).is_ok()
}

/// In the helper: prints the apps folder. Returns whether that worked.
#[must_use]
pub fn run_list() -> bool {
    let Ok(apps) = sohi_win::apps_folder() else {
        return false;
    };
    let mut out = std::io::stdout().lock();
    for (name, target) in apps {
        if format!("{name}{target}").contains(['\t', '\n', '\r']) {
            continue;
        }
        if writeln!(out, "{name}\t{target}").is_err() {
            return false;
        }
    }
    true
}

fn command() -> std::io::Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(NO_WINDOW);
    Ok(command)
}

/// Opens `target` with a helper. Returns whether that worked.
pub(crate) fn open(target: &str) -> std::io::Result<bool> {
    // What opens may come to the front if Sohi's pill is there now.
    sohi_win::shell::allow_foreground();
    let mut helper = command()?.arg(OPEN).arg(target).spawn()?;
    let deadline = Instant::now() + OPEN_WAIT;
    loop {
        match helper.try_wait()? {
            Some(status) => return Ok(status.success()),
            None if Instant::now() >= deadline => return Ok(true),
            None => std::thread::sleep(POLL),
        }
    }
}

/// Lists the apps folder with a helper; `None` if that did not work.
pub(crate) fn list() -> Option<Vec<(String, String)>> {
    let output = command()
        .ok()?
        .arg(LIST)
        .stdout(Stdio::piped())
        .output()
        .ok()?;
    let apps = parse(&String::from_utf8_lossy(&output.stdout));
    (output.status.success() && !apps.is_empty()).then_some(apps)
}

fn parse(list: &str) -> Vec<(String, String)> {
    list.lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, target)| (name.to_owned(), target.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_read_back() {
        let list = "Notepad\tshell:AppsFolder\\x!App\nno tab\nPaint\tshell:AppsFolder\\y\r\n";
        assert_eq!(
            parse(list),
            [
                ("Notepad".to_owned(), "shell:AppsFolder\\x!App".to_owned()),
                ("Paint".to_owned(), "shell:AppsFolder\\y".to_owned()),
            ]
        );
    }
}
