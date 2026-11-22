//! Raise-to-wake corner polling (spec §4.7): samples the cursor on the
//! poll timer and adapts the period to how close it is to the corner.

use sohi_core::geometry::RectPx;
use sohi_core::wake::{POLL_FAR, POLL_TOLERANCE, poll_period};
use sohi_win::{AppWindow, CursorPoller};

/// Corner wake detection.
pub struct Corner {
    poller: CursorPoller,
}

impl Corner {
    /// Starts polling at the far rate (spec §5.8 step 3).
    pub fn start(window: &AppWindow) -> Self {
        let corner = Self {
            poller: CursorPoller::new(window, POLL_TOLERANCE),
        };
        corner.set_period(Some(POLL_FAR));
        corner
    }

    /// Handles a poll tick: samples the cursor against `monitor`
    /// (`rcMonitor` of the primary display) and retimes the next poll.
    pub fn on_poll(&mut self, monitor: RectPx) {
        let Some(cursor) = sohi_win::cursor_pos() else {
            // Secure desktop or similar: keep the slow rate.
            self.set_period(Some(POLL_FAR));
            return;
        };
        self.set_period(Some(poll_period(cursor, monitor)));
    }

    fn set_period(&self, period: Option<std::time::Duration>) {
        let before = self.poller.period();
        if let Err(err) = self.poller.set_period(period) {
            log::warn!("cursor poll timer failed; corner wake is off: {err}");
        } else if before != period {
            log::debug!("cursor poll period: {period:?}");
        }
    }
}
