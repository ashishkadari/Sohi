//! Primary monitor geometry and DPI (spec §4.1).

use sohi_core::geometry::{BASE_DPI, RectPx};
use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

use crate::WinError;

/// Rectangles and DPI of a display, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Full monitor bounds (`rcMonitor`); the wake hot zone is based on this.
    pub monitor: RectPx,
    /// Area not covered by taskbars (`rcWork`); the head is placed in this.
    pub work: RectPx,
    /// Effective DPI (96 at 100% scaling).
    pub dpi: u32,
}

fn to_rect(r: RECT) -> RectPx {
    RectPx {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Reads the primary monitor's geometry and effective DPI.
///
/// Cheap (three system calls), so callers re-query on every reveal and on
/// every display-change notification instead of caching.
///
/// # Errors
/// Returns [`WinError`] if the monitor info cannot be read.
pub fn primary_monitor() -> Result<MonitorInfo, WinError> {
    // SAFETY: (0, 0) is always on the primary monitor; DEFAULTTOPRIMARY
    // guarantees a valid handle.
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    // SAFETY: `info` is a valid MONITORINFO with cbSize set.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        return Err(WinError::last("GetMonitorInfoW"));
    }
    let (mut dpi_x, mut dpi_y) = (BASE_DPI, BASE_DPI);
    // SAFETY: valid monitor handle and out-pointers. On failure the values
    // stay at 96 (100%), which only makes the widget smaller.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) };
    Ok(MonitorInfo {
        monitor: to_rect(info.rcMonitor),
        work: to_rect(info.rcWork),
        dpi: dpi_x,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_monitor_is_sane() {
        let m = primary_monitor().unwrap();
        assert!(m.monitor.width() > 0 && m.monitor.height() > 0);
        assert!(m.dpi >= 96);
        // The work area lies inside the monitor.
        assert!(m.work.left >= m.monitor.left && m.work.right <= m.monitor.right);
        assert!(m.work.top >= m.monitor.top && m.work.bottom <= m.monitor.bottom);
    }
}
