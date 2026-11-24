//! Owned GDI objects (brushes and fonts) for normal windows.

use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteObject, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GetDC, GetTextMetricsW,
    HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject, TEXTMETRICW,
};
use windows::core::w;

use crate::WinError;

/// `0xRRGGBB` as a GDI colour (which is `0x00BBGGRR`).
#[must_use]
pub(crate) fn rgb(rrggbb: u32) -> COLORREF {
    let [_, r, g, b] = rrggbb.to_be_bytes();
    COLORREF(u32::from_le_bytes([r, g, b, 0]))
}

/// A solid brush, deleted on drop.
pub(crate) struct Brush(HBRUSH);

impl Brush {
    pub(crate) fn solid(color: COLORREF) -> Result<Self, WinError> {
        // SAFETY: no preconditions.
        let brush = unsafe { CreateSolidBrush(color) };
        if brush.is_invalid() {
            return Err(WinError::last("CreateSolidBrush"));
        }
        Ok(Self(brush))
    }

    pub(crate) fn raw(&self) -> HBRUSH {
        self.0
    }
}

impl Drop for Brush {
    fn drop(&mut self) {
        // SAFETY: we own the brush and it is not selected into any DC
        // (window procedures only hand it out for the current message).
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

/// A font, deleted on drop.
pub(crate) struct Font(HFONT);

impl Font {
    /// Segoe UI Variable Text, 10.5 pt at `dpi` (spec §4.6). Windows
    /// substitutes a close match if the face is missing.
    pub(crate) fn ui(dpi: u32) -> Result<Self, WinError> {
        Self::sized(dpi, 21, false)
    }

    /// Segoe UI Variable Text in `half_points` at `dpi`.
    pub(crate) fn sized(dpi: u32, half_points: u32, bold: bool) -> Result<Self, WinError> {
        // Pixels = pt × dpi / 72, rounded.
        let px = (half_points * dpi + 72) / 144;
        let weight = if bold { FW_SEMIBOLD } else { FW_NORMAL };
        let height = -i32::try_from(px).unwrap_or(14);
        // SAFETY: the face name is a static NUL-terminated string.
        let font = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                weight.0.cast_signed(),
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                u32::from(DEFAULT_PITCH.0 | FF_DONTCARE.0),
                w!("Segoe UI Variable Text"),
            )
        };
        if font.is_invalid() {
            return Err(WinError::last("CreateFontW"));
        }
        Ok(Self(font))
    }

    pub(crate) fn raw(&self) -> HFONT {
        self.0
    }

    /// The font's line height in pixels, measured on `hwnd`'s DC.
    pub(crate) fn line_height(&self, hwnd: HWND) -> i32 {
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: GetDC/ReleaseDC pair; the font is selected only for the
        // measurement and the previous object is restored.
        unsafe {
            let dc = GetDC(Some(hwnd));
            let old = self.select(dc);
            let _ = GetTextMetricsW(dc, &raw mut metrics);
            Self::restore(dc, old);
            ReleaseDC(Some(hwnd), dc);
        }
        if metrics.tmHeight > 0 {
            metrics.tmHeight
        } else {
            18
        }
    }

    /// Selects the font into `dc`, returning what was selected before.
    ///
    /// # Safety
    /// `dc` must be a valid DC, and the previous object must be restored
    /// with [`Font::restore`] before the font is dropped.
    pub(crate) unsafe fn select(&self, dc: HDC) -> HGDIOBJ {
        // SAFETY: per the function contract.
        unsafe { SelectObject(dc, self.0.into()) }
    }

    /// Restores the object [`Font::select`] replaced.
    ///
    /// # Safety
    /// `dc` must be the DC passed to `select`.
    pub(crate) unsafe fn restore(dc: HDC, old: HGDIOBJ) {
        // SAFETY: per the function contract.
        unsafe {
            SelectObject(dc, old);
        }
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: we own the font; callers restore DCs before dropping it.
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_swaps_to_bgr() {
        assert_eq!(rgb(0xB0_12_2A), COLORREF(0x002A_12B0));
    }
}
