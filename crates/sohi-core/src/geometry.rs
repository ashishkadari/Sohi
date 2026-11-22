//! Placement math for the head widget (spec §4.1). Pure: callers pass in
//! the monitor rectangles and DPI.
//!
//! Sizes are in device-independent pixels (DIPs, 1/96 inch) and are scaled
//! to physical pixels with the target monitor's effective DPI.

/// Effective DPI at 100% scaling.
pub const BASE_DPI: u32 = 96;

/// Head window size in DIPs (frame, head, visor and shadow all fit inside).
pub const HEAD_WINDOW_DIP: (u32, u32) = (112, 84);

/// Gap between the head window and the work area's top-right corner, in DIPs.
pub const HEAD_MARGIN_DIP: u32 = 12;

/// A rectangle in physical screen pixels; `right` and `bottom` are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectPx {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

impl RectPx {
    /// Width in pixels (0 for an inverted rectangle).
    #[must_use]
    pub fn width(&self) -> u32 {
        u32::try_from(self.right - self.left).unwrap_or(0)
    }

    /// Height in pixels (0 for an inverted rectangle).
    #[must_use]
    pub fn height(&self) -> u32 {
        u32::try_from(self.bottom - self.top).unwrap_or(0)
    }

    /// Whether the point lies inside the rectangle.
    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Scales a DIP length to physical pixels at `dpi`, rounding to nearest.
#[must_use]
pub fn dip_to_px(dip: u32, dpi: u32) -> i32 {
    let px = (u64::from(dip) * u64::from(dpi) + u64::from(BASE_DPI / 2)) / u64::from(BASE_DPI);
    i32::try_from(px).unwrap_or(i32::MAX)
}

/// Where the head window goes: the top-right of the work area (so a
/// top-docked taskbar is respected), inset by [`HEAD_MARGIN_DIP`].
#[must_use]
pub fn head_window_rect(work: RectPx, dpi: u32) -> RectPx {
    let width = dip_to_px(HEAD_WINDOW_DIP.0, dpi);
    let height = dip_to_px(HEAD_WINDOW_DIP.1, dpi);
    let margin = dip_to_px(HEAD_MARGIN_DIP, dpi);
    let right = work.right - margin;
    let top = work.top + margin;
    RectPx {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: RectPx = RectPx {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1032,
    };

    #[test]
    fn dip_scaling_rounds_to_nearest() {
        assert_eq!(dip_to_px(112, 96), 112);
        assert_eq!(dip_to_px(112, 144), 168);
        assert_eq!(dip_to_px(112, 192), 224);
        // 12 DIP at 125% = 15 px; 1 DIP at 125% = 1.25 -> 1; at 175% = 1.75 -> 2.
        assert_eq!(dip_to_px(12, 120), 15);
        assert_eq!(dip_to_px(1, 120), 1);
        assert_eq!(dip_to_px(1, 168), 2);
    }

    #[test]
    fn head_sits_in_work_area_top_right_at_100_percent() {
        let r = head_window_rect(FHD, 96);
        assert_eq!(
            r,
            RectPx {
                left: 1920 - 12 - 112,
                top: 12,
                right: 1920 - 12,
                bottom: 12 + 84,
            }
        );
    }

    #[test]
    fn head_scales_with_dpi() {
        let r = head_window_rect(FHD, 192);
        assert_eq!((r.width(), r.height()), (224, 168));
        assert_eq!((r.right, r.top), (1920 - 24, 24));
    }

    #[test]
    fn head_respects_top_taskbar_and_offset_monitor() {
        // A monitor to the right of the primary with a 48 px top taskbar.
        let work = RectPx {
            left: 1920,
            top: 48,
            right: 4480,
            bottom: 1440,
        };
        let r = head_window_rect(work, 96);
        assert_eq!((r.right, r.top), (4480 - 12, 48 + 12));
    }

    #[test]
    fn head_never_crosses_the_work_area_right_edge() {
        for dpi in [96, 120, 144, 168, 192, 240, 288] {
            let r = head_window_rect(FHD, dpi);
            assert!(r.right <= FHD.right, "dpi {dpi}");
            assert!(r.top >= FHD.top, "dpi {dpi}");
        }
    }

    #[test]
    fn rect_helpers() {
        let r = RectPx {
            left: 10,
            top: 20,
            right: 30,
            bottom: 25,
        };
        assert_eq!((r.width(), r.height()), (20, 5));
        assert!(r.contains(10, 20));
        assert!(!r.contains(30, 20));
        let inverted = RectPx {
            left: 5,
            top: 5,
            right: 0,
            bottom: 0,
        };
        assert_eq!((inverted.width(), inverted.height()), (0, 0));
    }
}
