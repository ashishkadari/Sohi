//! Cursor polling for raise-to-wake (spec §4.7).
//!
//! A `SetCoalescableTimer` on the hidden app window posts `WM_TIMER`, which
//! becomes [`WinEvent::CursorPoll`](crate::WinEvent::CursorPoll). The app
//! samples [`cursor_pos`] and [`mouse_button_down`] and picks the next
//! period. Polling stops entirely while suspended (`set_period(None)`).

use std::cell::Cell;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, KillTimer, SetCoalescableTimer};

use crate::{AppWindow, WinError};

/// `WM_TIMER` id of the cursor poll on the app window.
pub(crate) const CURSOR_TIMER_ID: usize = 0x5348_4350; // "SHCP"

/// The cursor poll timer. Owned by the UI thread; killed on drop.
#[derive(Debug)]
pub struct CursorPoller {
    hwnd: HWND,
    tolerance_ms: u32,
    period: Cell<Option<Duration>>,
}

impl CursorPoller {
    /// A stopped poller that will post to `window`; the OS may delay each
    /// tick by up to `tolerance` to coalesce wakeups.
    #[must_use]
    pub fn new(window: &AppWindow, tolerance: Duration) -> Self {
        Self {
            hwnd: window.raw(),
            tolerance_ms: millis(tolerance),
            period: Cell::new(None),
        }
    }

    /// Polls every `period`, or stops for `None`. Does nothing if the period
    /// is unchanged, so calling it after every sample is cheap.
    ///
    /// # Errors
    /// Returns [`WinError`] if the timer cannot be set; polling is then
    /// stopped.
    pub fn set_period(&self, period: Option<Duration>) -> Result<(), WinError> {
        if period == self.period.get() {
            return Ok(());
        }
        self.period.set(period);
        match period {
            Some(p) => {
                // SAFETY: our window, a fixed id; no TIMERPROC, so the tick
                // arrives as WM_TIMER. Re-setting an existing id replaces it.
                let id = unsafe {
                    SetCoalescableTimer(
                        Some(self.hwnd),
                        CURSOR_TIMER_ID,
                        millis(p).max(1),
                        None,
                        self.tolerance_ms,
                    )
                };
                if id == 0 {
                    self.period.set(None);
                    return Err(WinError::last("SetCoalescableTimer"));
                }
            }
            None => self.kill(),
        }
        Ok(())
    }

    /// The current period, or `None` while stopped.
    #[must_use]
    pub fn period(&self) -> Option<Duration> {
        self.period.get()
    }

    fn kill(&self) {
        // SAFETY: our window and id; killing a missing timer fails harmlessly.
        let _ = unsafe { KillTimer(Some(self.hwnd), CURSOR_TIMER_ID) };
    }
}

impl Drop for CursorPoller {
    fn drop(&mut self) {
        self.kill();
    }
}

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

/// The cursor position in physical screen pixels, or `None` if it cannot
/// be read (e.g. on the secure desktop).
#[must_use]
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut pt = POINT::default();
    // SAFETY: `pt` is a valid out-pointer.
    unsafe { GetCursorPos(&raw mut pt) }.ok()?;
    Some((pt.x, pt.y))
}

/// Whether the left, right or middle mouse button is held right now.
#[must_use]
pub fn mouse_button_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
        .into_iter()
        .any(key_down)
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: no preconditions. The high bit is "down now".
    let state = unsafe { GetAsyncKeyState(i32::from(vk.0)) };
    state < 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventQueue;

    #[test]
    fn cursor_is_readable() {
        // Interactive desktops always have a cursor position.
        assert!(cursor_pos().is_some());
    }

    #[test]
    fn period_changes_are_tracked() {
        let window = AppWindow::create("Sohi.Test.Cursor", 0xC124, EventQueue::new()).unwrap();
        let poller = CursorPoller::new(&window, Duration::from_millis(32));
        assert_eq!(poller.period(), None);
        poller.set_period(Some(Duration::from_millis(250))).unwrap();
        assert_eq!(poller.period(), Some(Duration::from_millis(250)));
        poller.set_period(Some(Duration::from_millis(50))).unwrap();
        assert_eq!(poller.period(), Some(Duration::from_millis(50)));
        poller.set_period(None).unwrap();
        assert_eq!(poller.period(), None);
    }
}
