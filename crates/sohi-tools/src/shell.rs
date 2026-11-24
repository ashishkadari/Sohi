//! Running a command (spec §5.6). Only in builds with the `shell` feature,
//! and never without asking.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::CallError;

/// A command gets this long.
const TIMEOUT: Duration = Duration::from_secs(15);
/// How much output is kept.
const MAX_OUTPUT: usize = 4 * 1024;
/// The user must be able to read the whole command in the question.
const MAX_COMMAND: usize = 300;
/// `CREATE_NO_WINDOW`.
const NO_WINDOW: u32 = 0x0800_0000;
const POLL: Duration = Duration::from_millis(15);

/// Checks that the question can show all of `command`.
pub(crate) fn check(command: &str) -> Result<(), CallError> {
    if command.chars().count() > MAX_COMMAND {
        let why = format!("\"command\" is longer than {MAX_COMMAND} characters");
        return Err(CallError::BadArguments(why));
    }
    if command != crate::call::visible(command) {
        let why = "\"command\" must be one line of visible characters".to_owned();
        return Err(CallError::BadArguments(why));
    }
    Ok(())
}

/// What a command did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code; `None` if it was stopped.
    pub code: Option<i32>,
    /// What it wrote, cut to 4 KB.
    pub output: String,
}

/// Runs `command` with `cmd.exe`, without a window or input. It and
/// everything it started is stopped after 15 s.
///
/// # Errors
/// Returns why it could not be started.
pub fn run(command: &str) -> Result<Ran, String> {
    run_for(command, TIMEOUT)
}

fn run_for(command: &str, timeout: Duration) -> Result<Ran, String> {
    let cannot = |err: &dyn std::fmt::Display| {
        log::warn!("cannot run a command: {err}");
        "Windows could not start it".to_owned()
    };
    let job = sohi_win::Job::new().map_err(|e| cannot(&e))?;
    let mut child = Command::new("cmd.exe")
        .raw_arg(format!("/d /s /c \"{command}\""))
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| cannot(&e))?;
    if let Err(err) = job.adopt(&child) {
        let _ = child.kill();
        return Err(cannot(&err));
    }
    let outputs = [
        child.stdout.take().map(collect),
        child.stderr.take().map(collect),
    ];
    let code = wait(&mut child, Instant::now() + timeout);
    // Stops whatever is left of it, which also ends its output.
    drop(job);
    let mut output = String::new();
    for text in outputs.into_iter().flatten().filter_map(|o| o.recv().ok()) {
        output.push_str(&String::from_utf8_lossy(&text));
    }
    let mut cut = output.len().min(MAX_OUTPUT);
    while !output.is_char_boundary(cut) {
        cut -= 1;
    }
    output.truncate(cut);
    Ok(Ran { code, output })
}

/// Waits until `deadline`; `None` if it is still running then.
fn wait(child: &mut Child, deadline: Instant) -> Option<i32> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => return None,
        }
    }
}

/// Reads all of `pipe` on its own thread, keeping the first 4 KB.
fn collect(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (done, output) = channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0; 1024];
        while let Ok(n @ 1..) = pipe.read(&mut buffer) {
            let room = MAX_OUTPUT.saturating_sub(kept.len());
            kept.extend_from_slice(buffer.get(..n.min(room)).unwrap_or_default());
        }
        let _ = done.send(kept);
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_reports() {
        let ran = run("echo out& echo \"err\" 1>&2& exit 3").unwrap();
        assert_eq!(ran.code, Some(3));
        assert!(ran.output.contains("out"), "{ran:?}");
        assert!(ran.output.contains("\"err\""), "{ran:?}");
    }

    #[test]
    fn output_is_cut() {
        let ran = run("for /l %i in (1,1,2000) do @echo 0123456789").unwrap();
        assert_eq!(ran.code, Some(0));
        assert_eq!(ran.output.len(), MAX_OUTPUT);
    }

    #[test]
    fn is_stopped_with_everything_it_started() {
        let began = Instant::now();
        let ran = run_for("ping -n 30 127.0.0.1", Duration::from_millis(300)).unwrap();
        assert_eq!(ran.code, None);
        assert!(began.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn only_what_can_be_shown_may_be_asked() {
        assert!(check("dir /b").is_ok());
        assert!(check("dir\r\nformat c:").is_err());
        assert!(check("dir \u{202E}txt").is_err());
        assert!(check(&"x".repeat(MAX_COMMAND + 1)).is_err());
    }
}
