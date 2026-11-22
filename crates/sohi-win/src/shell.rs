//! Shell helpers: opening files and folders (`ShellExecuteW`) and the
//! user's do-not-disturb state (`SHQueryUserNotificationState`).

use std::path::Path;

use windows::Win32::UI::Shell::{
    QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
    SHQueryUserNotificationState, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

use crate::WinError;

/// `SE_ERR_NOASSOC`: no application is associated with the file type.
const SE_ERR_NOASSOC: isize = 31;

/// Outcome of [`open_path`] when it fails.
#[derive(Debug)]
pub enum OpenError {
    /// No program is registered for this file type.
    NoAssociation,
    /// Any other failure.
    Failed(WinError),
}

/// Opens `path` with its default handler (Explorer for folders).
///
/// # Errors
/// [`OpenError::NoAssociation`] if no program handles the file type.
pub fn open_path(path: &Path) -> Result<(), OpenError> {
    let target = HSTRING::from(path);
    // SAFETY: all strings are valid wide strings that outlive the call; no
    // parent window, parameters or working directory.
    let result = unsafe { ShellExecuteW(None, w!("open"), &target, None, None, SW_SHOWNORMAL) };
    check(result.0 as isize)
}

/// Runs `program` with `path` as its only argument (e.g. Notepad).
///
/// # Errors
/// [`OpenError::Failed`] if the program cannot be started.
pub fn open_with(program: &str, path: &Path) -> Result<(), OpenError> {
    let program = HSTRING::from(program);
    // Quote the path so spaces survive argument splitting.
    let args = HSTRING::from(format!("\"{}\"", path.display()));
    // SAFETY: valid wide strings that outlive the call.
    let result = unsafe { ShellExecuteW(None, w!("open"), &program, &args, None, SW_SHOWNORMAL) };
    check(result.0 as isize)
}

fn check(code: isize) -> Result<(), OpenError> {
    // ShellExecuteW returns a value > 32 on success.
    match code {
        c if c > 32 => Ok(()),
        SE_ERR_NOASSOC => Err(OpenError::NoAssociation),
        _ => Err(OpenError::Failed(WinError::last("ShellExecuteW"))),
    }
}

/// Why the user should not be interrupted right now, if they should not:
/// a full-screen Direct3D app, a full-screen "busy" app (video, games
/// without D3D exclusive mode) or presentation mode (spec §4.7). Corner
/// wake is suppressed while this is `Some`; hotkeys still work.
#[must_use]
pub fn do_not_disturb() -> Option<&'static str> {
    // SAFETY: no preconditions; the wrapper supplies the out-pointer.
    let state = unsafe { SHQueryUserNotificationState() }.ok()?;
    quiet_reason(state)
}

fn quiet_reason(state: QUERY_USER_NOTIFICATION_STATE) -> Option<&'static str> {
    match state {
        QUNS_RUNNING_D3D_FULL_SCREEN => Some("full-screen Direct3D app"),
        QUNS_BUSY => Some("full-screen app"),
        QUNS_PRESENTATION_MODE => Some("presentation mode"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Shell::{
        QUNS_ACCEPTS_NOTIFICATIONS, QUNS_NOT_PRESENT, QUNS_QUIET_TIME,
    };

    #[test]
    fn only_full_screen_and_presentations_are_quiet() {
        assert!(quiet_reason(QUNS_RUNNING_D3D_FULL_SCREEN).is_some());
        assert!(quiet_reason(QUNS_BUSY).is_some());
        assert!(quiet_reason(QUNS_PRESENTATION_MODE).is_some());
        // Focus assist ("quiet time"), a locked screen or the normal
        // desktop do not block the corner.
        assert!(quiet_reason(QUNS_ACCEPTS_NOTIFICATIONS).is_none());
        assert!(quiet_reason(QUNS_QUIET_TIME).is_none());
        assert!(quiet_reason(QUNS_NOT_PRESENT).is_none());
    }

    #[test]
    fn query_does_not_fail() {
        // The answer depends on what else is on screen; it must not panic.
        let _ = do_not_disturb();
    }
}
