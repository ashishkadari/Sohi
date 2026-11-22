//! Raise-to-wake corner detection (spec §4.7), as pure geometry and timing.
//!
//! `sohi-win` polls the cursor; this module decides how often to poll and,
//! from each sample, whether the pointer has dwelt in the hot zone long
//! enough to wake.

use std::time::{Duration, Instant};

use crate::config::WakeConfig;
use crate::geometry::RectPx;

/// Poll period while the cursor is far from the corner (4 Hz).
pub const POLL_FAR: Duration = Duration::from_millis(250);
/// Poll period while the cursor is near the corner (20 Hz).
pub const POLL_NEAR: Duration = Duration::from_millis(50);
/// How much the OS may delay a poll to coalesce it with other timers.
pub const POLL_TOLERANCE: Duration = Duration::from_millis(32);
/// "Near" means within this many physical pixels of the corner.
pub const NEAR_RADIUS_PX: i32 = 300;

/// The corner wake tunables from `[wake]` (spec §4.7, §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CornerSettings {
    /// The hot zone is this many physical pixels square, at the corner.
    pub zone_px: i32,
    /// How long the pointer must stay in the hot zone to wake.
    pub dwell: Duration,
    /// No corner wake for this long after the head finished retracting.
    pub cooldown: Duration,
}

impl Default for CornerSettings {
    fn default() -> Self {
        Self::from_config(&WakeConfig::default())
    }
}

impl CornerSettings {
    /// Reads `[wake]`, clamping each value to a usable range: a 1–64 px
    /// zone, a dwell of at most 5 s and a cooldown of at most 60 s.
    #[must_use]
    pub fn from_config(wake: &WakeConfig) -> Self {
        let ms = |v: u32, max: u32| Duration::from_millis(u64::from(v.min(max)));
        Self {
            zone_px: i32::try_from(wake.corner_size_px.clamp(1, 64)).unwrap_or(8),
            dwell: ms(wake.dwell_ms, 5000),
            cooldown: ms(wake.cooldown_ms, 60_000),
        }
    }
}

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
    /// in the zone, with no button held, for at least `dwell`, and this
    /// visit has not been [`used`](Self::use_up) yet.
    pub fn sample(
        &mut self,
        now: Instant,
        in_zone: bool,
        button_down: bool,
        dwell: Duration,
    ) -> bool {
        if !in_zone || button_down {
            *self = Self::default();
            return false;
        }
        let entered = *self.entered.get_or_insert(now);
        !self.used && now.saturating_duration_since(entered) >= dwell
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

/// Whether `cursor` is inside the `zone_px`-square hot zone at the
/// top-right of `monitor`.
#[must_use]
pub fn in_hot_zone(cursor: (i32, i32), monitor: RectPx, zone_px: i32) -> bool {
    let (x, y) = cursor;
    x >= monitor.right - zone_px
        && x < monitor.right
        && y >= monitor.top
        && y < monitor.top + zone_px
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
        assert!(in_hot_zone((1919, 0), SCREEN, 8));
        assert!(in_hot_zone((1912, 7), SCREEN, 8));
        assert!(!in_hot_zone((1911, 0), SCREEN, 8));
        assert!(!in_hot_zone((1919, 8), SCREEN, 8));
        assert!(!in_hot_zone((1920, 0), SCREEN, 8), "next monitor");
        assert!(!in_hot_zone((1919, -1), SCREEN, 8), "monitor above");
    }

    #[test]
    fn hot_zone_follows_a_monitor_not_at_the_origin() {
        let m = RectPx {
            left: -2560,
            top: -300,
            right: 0,
            bottom: 1140,
        };
        assert!(in_hot_zone((-1, -300), m, 8));
        assert!(!in_hot_zone((1919, 0), m, 8));
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;
    const DWELL: Duration = Duration::from_millis(350);

    #[test]
    fn dwell_needs_continuous_presence() {
        let t0 = Instant::now();
        let mut d = Dwell::default();
        assert!(!d.sample(t0, true, false, DWELL));
        assert!(!d.sample(t0 + MS(300), true, false, DWELL));
        assert!(d.sample(t0 + MS(350), true, false, DWELL));
        // Leaving resets presence.
        let mut d = Dwell::default();
        d.sample(t0, true, false, DWELL);
        d.sample(t0 + MS(200), false, false, DWELL);
        assert!(!d.sample(t0 + MS(250), true, false, DWELL));
        assert!(!d.sample(t0 + MS(550), true, false, DWELL));
        assert!(d.sample(t0 + MS(600), true, false, DWELL));
    }

    #[test]
    fn a_held_button_resets_and_blocks() {
        let t0 = Instant::now();
        let mut d = Dwell::default();
        assert!(!d.sample(t0, true, true, DWELL));
        assert!(!d.sample(t0 + MS(1000), true, true, DWELL), "dragging");
        assert!(
            !d.sample(t0 + MS(1100), true, false, DWELL),
            "released: starts now"
        );
        assert!(d.sample(t0 + MS(1450), true, false, DWELL));
        d.sample(t0 + MS(1460), true, true, DWELL);
        assert!(
            !d.sample(t0 + MS(1470), true, false, DWELL),
            "press reset it"
        );
    }

    #[test]
    fn a_used_dwell_needs_a_new_visit() {
        let t0 = Instant::now();
        let mut d = Dwell::default();
        d.sample(t0, true, false, DWELL);
        assert!(d.sample(t0 + MS(400), true, false, DWELL));
        d.use_up();
        assert!(!d.sample(t0 + MS(5000), true, false, DWELL), "lingering");
        d.sample(t0 + MS(5050), false, false, DWELL);
        d.sample(t0 + MS(5100), true, false, DWELL);
        assert!(d.sample(t0 + MS(5450), true, false, DWELL));
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
                    d.sample(t, inside, false, DWELL)
                });
                assert!(!woke, "phase {phase} stay {stay}");
            }
        }
    }

    #[test]
    fn settings_follow_the_config_within_limits() {
        assert_eq!(
            CornerSettings::default(),
            CornerSettings {
                zone_px: 8,
                dwell: DWELL,
                cooldown: MS(1500),
            }
        );
        let wild = WakeConfig {
            corner_size_px: 0,
            dwell_ms: 99_999,
            cooldown_ms: u32::MAX,
            ..WakeConfig::default()
        };
        let s = CornerSettings::from_config(&wild);
        assert_eq!((s.zone_px, s.dwell, s.cooldown), (1, MS(5000), MS(60_000)));
        assert!(in_hot_zone((1919, 0), SCREEN, 1));
        assert!(!in_hot_zone((1918, 0), SCREEN, 1));
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
