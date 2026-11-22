//! Timers on the hidden app window: `WM_TIMER` becomes
//! [`WinEvent::Timer`](crate::WinEvent::Timer). They tick until dropped;
//! the app drops one-shot timers when they first fire.

use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::{AppWindow, WinError};

/// Something that can own app-window timers. Copy it freely; it is only
/// valid on the UI thread while the app window exists.
#[derive(Debug, Clone, Copy)]
pub struct TimerHost(HWND);

impl TimerHost {
    /// The host for `window`.
    #[must_use]
    pub fn new(window: &AppWindow) -> Self {
        Self(window.raw())
    }

    /// Starts timer `id`, which fires every `period` until dropped. Ids
    /// must be small app-chosen numbers (the cursor poll uses its own).
    ///
    /// # Errors
    /// Returns [`WinError`] if the timer cannot be created.
    pub fn start(self, id: usize, period: Duration) -> Result<AppTimer, WinError> {
        let ms = u32::try_from(period.as_millis()).unwrap_or(u32::MAX).max(1);
        // SAFETY: the app window lives for the whole message loop; no
        // callback, so the timer posts WM_TIMER.
        if unsafe { SetTimer(Some(self.0), id, ms, None) } == 0 {
            return Err(WinError::last("SetTimer"));
        }
        Ok(AppTimer { hwnd: self.0, id })
    }
}

/// A running app-window timer, killed on drop.
#[derive(Debug)]
pub struct AppTimer {
    hwnd: HWND,
    id: usize,
}

impl AppTimer {
    /// The id it fires with.
    #[must_use]
    pub fn id(&self) -> usize {
        self.id
    }
}

impl Drop for AppTimer {
    fn drop(&mut self) {
        // SAFETY: killing our own timer; harmless if the window is gone.
        let _ = unsafe { KillTimer(Some(self.hwnd), self.id) };
    }
}
