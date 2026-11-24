//! The shell, from a short-lived process.
//!
//! Opening something and listing the apps folder each load about 36 system
//! DLLs, which then stay in the process for good (some 20 MB of working
//! set). So Sohi starts itself with [`OPEN`] or [`LIST`] for that: the
//! DLLs go when the helper ends. The same goes for the media controls
//! ([`MEDIA`]).

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use sohi_win::{MediaCommand, MediaResult};

/// Argument: open the target that follows.
pub const OPEN: &str = "--open";
/// Argument: print the apps folder, one app a line: name, tab, target.
pub const LIST: &str = "--list-apps";
/// Argument: do to the players what the next argument says.
pub const MEDIA: &str = "--media";

/// `CREATE_NO_WINDOW`.
const NO_WINDOW: u32 = 0x0800_0000;
/// Opening can wait for the user (an administrator prompt). It is taken to
/// have worked after this long.
const OPEN_WAIT: Duration = Duration::from_secs(8);
/// A player that takes longer than this hangs.
const MEDIA_WAIT: Duration = Duration::from_secs(8);
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

/// In the helper: controls the players. Returns the exit code that tells
/// Sohi what came of it, 1 if `action` is unknown or nothing worked.
#[must_use]
pub fn run_media(action: &str) -> u8 {
    COMMANDS
        .iter()
        .find(|(name, _)| *name == action)
        .and_then(|(_, command)| sohi_win::control_media(*command).ok())
        .and_then(|result| RESULTS.iter().find(|(_, r)| *r == result))
        .map_or(1, |(code, _)| *code)
}

const COMMANDS: [(&str, MediaCommand); 4] = [
    ("play", MediaCommand::Play),
    ("pause", MediaCommand::Pause),
    ("next", MediaCommand::Next),
    ("prev", MediaCommand::Previous),
];

/// Exit codes.
const RESULTS: [(u8, MediaResult); 4] = [
    (0, MediaResult::Done),
    (10, MediaResult::Already),
    (11, MediaResult::NoPlayer),
    (12, MediaResult::Ignored),
];

/// Controls the players with a helper; `None` if that did not work.
pub(crate) fn media(media: MediaCommand) -> std::io::Result<Option<MediaResult>> {
    let Some((name, _)) = COMMANDS.iter().find(|(_, c)| *c == media) else {
        return Ok(None);
    };
    let mut helper = command()?.arg(MEDIA).arg(name).spawn()?;
    let code = wait(&mut helper, MEDIA_WAIT)?.and_then(|status| status.code());
    if code.is_none() {
        // It hangs: nothing more may happen later.
        let _ = helper.kill();
    }
    Ok(RESULTS
        .iter()
        .find(|(c, _)| code == Some(i32::from(*c)))
        .map(|(_, result)| *result))
}

/// Waits for a helper to end; `None` if it has not after `limit`.
fn wait(helper: &mut Child, limit: Duration) -> std::io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + limit;
    loop {
        match helper.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None if Instant::now() >= deadline => return Ok(None),
            None => std::thread::sleep(POLL),
        }
    }
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
    Ok(wait(&mut helper, OPEN_WAIT)?.is_none_or(|status| status.success()))
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
