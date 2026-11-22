//! Raise-to-wake corner detection (spec §4.7), as pure geometry and timing.
//!
//! `sohi-win` polls the cursor; this module decides how often to poll and,
//! from each sample, whether the pointer has dwelt in the hot zone long
//! enough to wake.

use std::time::Duration;

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
