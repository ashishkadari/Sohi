//! The hidden application window.
//!
//! It is never shown. It receives app-wide messages: the second-instance
//! wake message, display and work-area broadcasts, the tray callback and
//! `TaskbarCreated`.
//! Destroying it (`WM_CLOSE`) ends the message loop.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, PostQuitMessage, SPI_SETWORKAREA, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_SETTINGCHANGE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::hwnd::{OwnedHwnd, WindowSpec, WndHandler, create_window, signed_words};
use crate::tray::TRAY_CALLBACK;
use crate::{EventQueue, WinError, WinEvent, register_window_message};

/// Tray notification codes (low word of `lParam`, version 4).
const NIN_SELECT: u32 = 0x0400;
const NIN_KEYSELECT: u32 = 0x0401;
const WM_CONTEXTMENU: u32 = 0x007B;

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
        let taskbar_created = register_window_message("TaskbarCreated")?;
        let hwnd = create_window(
            &spec,
            Box::new(AppHandler {
                wake_message,
                taskbar_created,
                queue,
            }),
        )?;
        Ok(Self { hwnd })
    }

    pub(crate) fn raw(&self) -> HWND {
        self.hwnd.raw()
    }

    /// Returns a `Send` reference for posting to this window from other threads.
    #[must_use]
    pub fn remote(&self) -> RemoteWindow {
        RemoteWindow(self.hwnd.raw().0 as usize)
    }
}

struct AppHandler {
    wake_message: u32,
    taskbar_created: u32,
    queue: EventQueue,
}

impl AppHandler {
    fn on_tray(&self, wparam: WPARAM, lparam: LPARAM) {
        let event = u32::try_from(lparam.0 & 0xFFFF).unwrap_or(0);
        let (x, y) = signed_words(wparam.0.cast_signed());
        match event {
            WM_CONTEXTMENU => self.queue.push(WinEvent::TrayMenu { x, y }),
            NIN_SELECT | NIN_KEYSELECT => self.queue.push(WinEvent::TrayActivated),
            _ => {}
        }
    }
}

impl WndHandler for AppHandler {
    fn on_message(&self, _hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            TRAY_CALLBACK => {
                self.on_tray(wparam, lparam);
                Some(LRESULT(0))
            }
            m if m == self.taskbar_created => {
                self.queue.push(WinEvent::TaskbarCreated);
                Some(LRESULT(0))
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                self.queue.push(WinEvent::DisplayChanged);
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE if wparam.0 == SPI_SETWORKAREA.0 as usize => {
                self.queue.push(WinEvent::DisplayChanged);
                Some(LRESULT(0))
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    fn send(window: &AppWindow, msg: u32, wparam: usize, lparam: isize) {
        // SAFETY: in-process send to a window created on this thread; the
        // only pointer argument (WM_DPICHANGED's RECT) outlives the call.
        unsafe {
            SendMessageW(
                window.hwnd.raw(),
                msg,
                Some(WPARAM(wparam)),
                Some(LPARAM(lparam)),
            )
        };
    }

    #[test]
    fn display_messages_become_display_changed() {
        let queue = EventQueue::new();
        let window = AppWindow::create("Sohi.Test.App", 0xC123, queue.clone()).unwrap();
        let suggested = RECT::default();
        send(&window, WM_DISPLAYCHANGE, 32, 0);
        send(&window, WM_SETTINGCHANGE, SPI_SETWORKAREA.0 as usize, 0);
        send(&window, WM_SETTINGCHANGE, 0x57, 0); // unrelated setting: ignored
        send(
            &window,
            WM_DPICHANGED,
            0x0090_0090,
            (&raw const suggested) as isize,
        );
        send(&window, 0xC123, 0, 0);
        let events: Vec<_> = std::iter::from_fn(|| queue.pop()).collect();
        assert_eq!(
            events,
            [
                WinEvent::DisplayChanged,
                WinEvent::DisplayChanged,
                WinEvent::DisplayChanged,
                WinEvent::WakeRequested,
            ]
        );
    }
}
