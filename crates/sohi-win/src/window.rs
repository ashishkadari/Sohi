//! The hidden application window.
//!
//! It is never shown. It receives app-wide messages: the second-instance
//! wake message now, and later the tray callback and system broadcasts.
//! Destroying it (`WM_CLOSE`) ends the message loop.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, PostQuitMessage, WM_CLOSE, WM_DESTROY, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_POPUP,
};

use crate::hwnd::{OwnedHwnd, WindowSpec, WndHandler, create_window};
use crate::{EventQueue, WinError, WinEvent};

/// The hidden top-level window that receives app-wide messages.
pub struct AppWindow {
    hwnd: OwnedHwnd,
}

impl AppWindow {
    /// Creates the window with class `class_name`.
    ///
    /// `wake_message` is the registered message that becomes
    /// [`WinEvent::WakeRequested`] on `queue`.
    ///
    /// # Errors
    /// Returns [`WinError`] if window creation fails.
    pub fn create(
        class_name: &str,
        wake_message: u32,
        queue: EventQueue,
    ) -> Result<Self, WinError> {
        let spec = WindowSpec {
            class_name,
            ex_style: WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            style: WS_POPUP,
        };
        let hwnd = create_window(
            &spec,
            Box::new(AppHandler {
                wake_message,
                queue,
            }),
        )?;
        Ok(Self { hwnd })
    }

    /// Returns a `Send` reference for posting to this window from other threads.
    #[must_use]
    pub fn remote(&self) -> RemoteWindow {
        RemoteWindow(self.hwnd.raw().0 as usize)
    }
}

struct AppHandler {
    wake_message: u32,
    queue: EventQueue,
}

impl WndHandler for AppHandler {
    fn on_message(
        &self,
        _hwnd: HWND,
        msg: u32,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            m if m == self.wake_message => {
                self.queue.push(WinEvent::WakeRequested);
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                // SAFETY: no preconditions.
                unsafe { PostQuitMessage(0) };
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

/// A thread-safe reference to an [`AppWindow`], usable only for posting.
#[derive(Debug, Clone, Copy)]
pub struct RemoteWindow(usize);

impl RemoteWindow {
    /// Asks the window to close, which ends the message loop.
    ///
    /// # Errors
    /// Returns [`WinError`] if the window no longer exists.
    pub fn post_close(self) -> Result<(), WinError> {
        let hwnd = HWND(self.0 as *mut c_void);
        // SAFETY: PostMessageW validates the window handle itself, and
        // WM_CLOSE carries no pointers.
        unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .map_err(|e| WinError::api("PostMessageW", &e))
    }
}
