//! Opening files and folders with the shell (`ShellExecuteW`).

use std::path::Path;

use windows::Win32::UI::Shell::ShellExecuteW;
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
