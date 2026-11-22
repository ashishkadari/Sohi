//! Raise-to-wake corner detection (spec §4.7): samples the cursor on the
//! poll timer, adapts the period to how close it is to the corner, and
//! decides when a dwell in the hot zone should wake Sohi.

use std::time::{Duration, Instant};

use sohi_core::geometry::RectPx;
use sohi_core::wake::{Dwell, Gate, POLL_FAR, POLL_TOLERANCE, in_hot_zone, poll_period};
use sohi_win::{AppWindow, CursorPoller};

/// Corner wake detection.
pub struct Corner {
    poller: CursorPoller,
    dwell: Dwell,
}

impl Corner {
    /// Starts polling at the far rate (spec §5.8 step 3).
    pub fn start(window: &AppWindow) -> Self {
        let corner = Self {
            poller: CursorPoller::new(window, POLL_TOLERANCE),
            dwell: Dwell::default(),
        };
        corner.set_period(Some(POLL_FAR));
        corner
    }

    /// Handles a poll tick: samples the cursor against `monitor`
    /// (`rcMonitor` of the primary display) and retimes the next poll.
    /// Returns `true` if the pointer dwelt in the hot zone with no button
    /// held, `gate` allows a wake now, and no full-screen app or
    /// presentation is running.
    pub fn on_poll(&mut self, monitor: RectPx, gate: impl FnOnce(Instant) -> Gate) -> bool {
        let now = Instant::now();
        let Some(cursor) = sohi_win::cursor_pos() else {
            // Secure desktop or similar: keep the slow rate.
            self.dwell = Dwell::default();
            self.set_period(Some(POLL_FAR));
            return false;
        };
        self.set_period(Some(poll_period(cursor, monitor)));
        let in_zone = in_hot_zone(cursor, monitor);
        // Only read the buttons when it matters.
        let button_down = in_zone && sohi_win::mouse_button_down();
        if !self.dwell.sample(now, in_zone, button_down) {
            return false;
        }
        match gate(now) {
            Gate::Wait => false,
            Gate::Deny => {
                self.dwell.use_up();
                false
            }
            Gate::Allow => {
                // Asked once per dwell; either way this visit is spent.
                self.dwell.use_up();
                match sohi_win::shell::do_not_disturb() {
                    Some(reason) => {
                        log::info!("corner wake suppressed: {reason}");
                        false
                    }
                    None => true,
                }
            }
        }
    }

    fn set_period(&self, period: Option<Duration>) {
        let before = self.poller.period();
        if let Err(err) = self.poller.set_period(period) {
            log::warn!("cursor poll timer failed; corner wake is off: {err}");
        } else if before != period {
            log::debug!("cursor poll period: {period:?}");
        }
    }
}
