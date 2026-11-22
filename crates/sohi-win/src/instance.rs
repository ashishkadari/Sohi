//! Single-instance guard and wake forwarding (spec §5.8 step 1).

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};
use windows::core::HSTRING;

use crate::{OwnedHandle, WinError};

/// Holds the named single-instance mutex for the life of the process.
#[derive(Debug)]
pub struct SingleInstance {
    _mutex: OwnedHandle,
}

impl SingleInstance {
    /// Creates the named mutex.
    ///
    /// Returns `Ok(None)` when another process already holds it.
    ///
    /// # Errors
    /// Returns [`WinError`] if the mutex cannot be created at all.
    pub fn acquire(name: &str) -> Result<Option<Self>, WinError> {
        let name = HSTRING::from(name);
        // SAFETY: `name` is a valid NUL-terminated wide string that outlives
        // the call; a null security descriptor means default security.
        let handle = unsafe { CreateMutexW(None, false, &name) }
            .map_err(|e| WinError::api("CreateMutexW", &e))?;
        // SAFETY: GetLastError has no preconditions. CreateMutexW succeeded,
        // so the last error is either 0 or ERROR_ALREADY_EXISTS.
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        // SAFETY: CreateMutexW returned a fresh handle that we now own.
        let mutex = unsafe { OwnedHandle::from_raw(handle) };
        Ok((!already_exists).then_some(Self { _mutex: mutex }))
    }
}

/// Registers (or looks up) a system-wide window message by name.
///
/// # Errors
/// Returns [`WinError`] if registration fails.
pub fn register_window_message(name: &str) -> Result<u32, WinError> {
    let name = HSTRING::from(name);
    // SAFETY: `name` is a valid NUL-terminated wide string.
    let id = unsafe { RegisterWindowMessageW(&name) };
    if id == 0 {
        return Err(WinError::last("RegisterWindowMessageW"));
    }
    Ok(id)
}

/// Posts `message` to the top-level window of class `class_name` owned by
/// the running instance.
///
/// # Errors
/// Returns [`WinError`] if no such window exists or posting fails.
pub fn forward_to_running_instance(class_name: &str, message: u32) -> Result<(), WinError> {
    let class = HSTRING::from(class_name);
    // SAFETY: `class` is a valid wide string; a null window name matches any.
    let hwnd =
        unsafe { FindWindowW(&class, None) }.map_err(|e| WinError::api("FindWindowW", &e))?;
    // SAFETY: posting to another process's window is safe for registered
    // messages; zero WPARAM/LPARAM carry no pointers.
    unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) }
        .map_err(|e| WinError::api("PostMessageW", &e))
}
