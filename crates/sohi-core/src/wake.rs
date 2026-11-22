//! Raise-to-wake corner detection (spec §4.7), as pure geometry and timing.
//!
//! `sohi-win` polls the cursor; this module decides how often to poll and,
//! from each sample, whether the pointer has dwelt in the hot zone long
//! enough to wake.

use std::time::{Duration, Instant};

use crate::geometry::RectPx;

/// Poll period while the cursor is far from the corner (4 Hz).
pub const POLL_FAR: Duration = Duration::from_millis(250);
/// Poll period while the cursor is near the corner (20 Hz).
pub const POLL_NEAR: Duration = Duration::from_millis(50);
/// How much the OS may delay a poll to coalesce it with other timers.
pub const POLL_TOLERANCE: Duration = Duration::from_millis(32);
/// "Near" means within this many physical pixels of the corner.
pub const NEAR_RADIUS_PX: i32 = 300;
/// The hot zone is this many physical pixels square, at the corner.
pub const HOT_ZONE_PX: i32 = 8;

/// How long the pointer must stay in the hot zone to wake.
pub const DWELL: Duration = Duration::from_millis(350);
/// No corner wake for this long after the head finished retracting.
pub const COOLDOWN: Duration = Duration::from_millis(1500);

/// Whether a completed dwell may wake Sohi right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Wake.
    Allow,
    /// Not yet (post-retract cooldown): keep dwelling and ask again.
    Wait,
    /// Not for this visit (already shown, or suppressed): the pointer must
    /// leave the hot zone and come back.
    Deny,
}

/// Continuous-presence tracking for the hot zone.
///
/// Presence starts at the first sample inside the zone and resets on any
/// sample outside it or with a mouse button held, so a drag through the
/// corner never counts. Once a dwell has been used (woken or suppressed)
/// it does not count again until the pointer leaves and comes back.
#[derive(Debug, Clone, Copy, Default)]
pub struct Dwell {
    entered: Option<Instant>,
    used: bool,
}

impl Dwell {
    /// Feeds one cursor sample. Returns `true` if the pointer has now been
    /// in the zone, with no button held, for at least [`DWELL`], and this
    /// visit has not been [`used`](Self::use_up) yet.
    pub fn sample(&mut self, now: Instant, in_zone: bool, button_down: bool) -> bool {
        if !in_zone || button_down {
            *self = Self::default();
            return false;
        }
        let entered = *self.entered.get_or_insert(now);
        !self.used && now.saturating_duration_since(entered) >= DWELL
    }

    /// Marks this visit as spent: it woke Sohi, or the wake was suppressed.
    pub fn use_up(&mut self) {
        self.used = true;
    }
}

/// The corner pixel the hot zone hangs from: the top-right pixel of
/// `monitor` (`rcMonitor`, so it is the true screen corner even with a
/// top taskbar).
#[must_use]
pub fn corner(monitor: RectPx) -> (i32, i32) {
    (monitor.right - 1, monitor.top)
}

/// Whether `cursor` is inside the 8×8 hot zone at the top-right of
/// `monitor`.
#[must_use]
pub fn in_hot_zone(cursor: (i32, i32), monitor: RectPx) -> bool {
    let (x, y) = cursor;
    x >= monitor.right - HOT_ZONE_PX
        && x < monitor.right
        && y >= monitor.top
        && y < monitor.top + HOT_ZONE_PX
}

/// How long to wait before the next cursor sample.
#[must_use]
pub fn poll_period(cursor: (i32, i32), monitor: RectPx) -> Duration {
    let (cx, cy) = corner(monitor);
    let dx = i64::from(cursor.0) - i64::from(cx);
    let dy = i64::from(cursor.1) - i64::from(cy);
    let r = i64::from(NEAR_RADIUS_PX);
    if dx * dx + dy * dy <= r * r {
        POLL_NEAR
    } else {
        POLL_FAR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RectPx = RectPx {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1200,
    };

    #[test]
    fn hot_zone_is_eight_pixels_at_the_top_right() {
        assert!(in_hot_zone((1919, 0), SCREEN));
        assert!(in_hot_zone((1912, 7), SCREEN));
        assert!(!in_hot_zone((1911, 0), SCREEN));
        assert!(!in_hot_zone((1919, 8), SCREEN));
        assert!(!in_hot_zone((1920, 0), SCREEN), "next monitor");
        assert!(!in_hot_zone((1919, -1), SCREEN), "monitor above");
    }

    #[test]
    fn hot_zone_follows_a_monitor_not_at_the_origin() {
        let m = RectPx {
            left: -2560,
            top: -300,
            right: 0,
            bottom: 1140,
        };
        assert!(in_hot_zone((-1, -300), m));
        assert!(!in_hot_zone((1919, 0), m));
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn dwell_needs_continuous_presence() {
        let t0 = Instant::now();
        let mut d = Dwell::default();
        assert!(!d.sample(t0, true, false));
        assert!(!d.sample(t0 + MS(300), true, false));
        assert!(d.sample(t0 + MS(350), true, false));
        // Leaving resets presence.
        let mut d = Dwell::default();
        d.sample(t0, true, false);
        d.sample(t0 + MS(200), false, false);
        assert!(!d.sample(t0 + MS(250), true, false));
        assert!(!d.sample(t0 + MS(550), true, false));
        assert!(d.sample(t0 + MS(600), true, false));
    }

    #[test]
    fn a_held_button_resets_and_blocks() {
        let t0 = Instant::now();
        let mut d = Dwell::default();
        assert!(!d.sample(t0, true, true));
        assert!(!d.sample(t0 + MS(1000), true, true), "dragging");
        assert!(
            !d.sample(t0 + MS(1100), true, false),
            "released: starts now"
        );
        assert!(d.sample(t0 + MS(1450), true, false));
        d.sample(t0 + MS(1460), true, true);
        assert!(!d.sample(t0 + MS(1470), true, false), "press reset it");
    }

    #[test]
    fn a_used_dwell_needs_a_new_visit() {
        let t0 = Instant::now();
        let mut d = Dwell::default();
        d.sample(t0, true, false);
        assert!(d.sample(t0 + MS(400), true, false));
        d.use_up();
        assert!(!d.sample(t0 + MS(5000), true, false), "lingering");
        d.sample(t0 + MS(5050), false, false);
        d.sample(t0 + MS(5100), true, false);
        assert!(d.sample(t0 + MS(5450), true, false));
    }

    /// Passing through in under 300 ms at the near poll rate never wakes,
    /// whatever the phase of the poll relative to the entry.
    #[test]
    fn a_quick_pass_never_wakes() {
        let t0 = Instant::now();
        for phase in 0..50 {
            for stay in [50, 150, 299] {
                let mut d = Dwell::default();
                let enter = t0 + MS(phase);
                let woke = (0..40).any(|i| {
                    let t = t0 + POLL_NEAR * i;
                    let inside = t >= enter && t < enter + MS(stay);
                    d.sample(t, inside, false)
                });
                assert!(!woke, "phase {phase} stay {stay}");
            }
        }
    }

    #[test]
    fn polls_fast_only_near_the_corner() {
        assert_eq!(poll_period((1919, 0), SCREEN), POLL_NEAR);
        assert_eq!(poll_period((1919 - 300, 0), SCREEN), POLL_NEAR);
        assert_eq!(poll_period((1919 - 301, 0), SCREEN), POLL_FAR);
        // Diagonal: 220² + 220² > 300².
        assert_eq!(poll_period((1919 - 220, 220), SCREEN), POLL_FAR);
        assert_eq!(poll_period((1919 - 200, 200), SCREEN), POLL_NEAR);
        assert_eq!(poll_period((0, 1199), SCREEN), POLL_FAR);
    }
}
