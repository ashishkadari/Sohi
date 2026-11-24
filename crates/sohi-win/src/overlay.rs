//! The click-through, top-most window the full mode animations play in
//! (spec §4.9). It exists only while one runs.

use sohi_core::geometry::RectPx;
use windows::Win32::Foundation::{POINT, SIZE};
use windows::Win32::UI::WindowsAndMessaging::{
    SW_SHOWNOACTIVATE, ShowWindow, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::WinError;
use crate::head::{Presented, update_layered};
use crate::hwnd::{OwnedHwnd, WindowSpec, WndHandler, create_window};
use crate::surface::{Surface, double_into};

/// Leaves every message to Windows.
struct Passive;

impl WndHandler for Passive {
    fn on_message(
        &self,
        _hwnd: windows::Win32::Foundation::HWND,
        _msg: u32,
        _wparam: windows::Win32::Foundation::WPARAM,
        _lparam: windows::Win32::Foundation::LPARAM,
    ) -> Option<windows::Win32::Foundation::LRESULT> {
        None
    }
}

/// The overlay. Destroyed on drop.
pub struct Overlay {
    hwnd: OwnedHwnd,
    surface: Surface,
    rect: RectPx,
    shown: bool,
}

impl Overlay {
    /// Creates the (hidden) overlay over `rect`.
    ///
    /// # Errors
    /// Returns [`WinError`] if the window or its bitmap cannot be created.
    pub fn create(rect: RectPx) -> Result<Self, WinError> {
        let spec = WindowSpec {
            class_name: "Sohi.Overlay",
            ex_style: WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE,
            style: WS_POPUP,
        };
        Ok(Self {
            surface: Surface::new(rect.width(), rect.height())?,
            hwnd: create_window(&spec, Box::new(Passive))?,
            rect,
            shown: false,
        })
    }

    /// The size of the frames [`Overlay::present`] takes: half the
    /// overlay's, rounded up.
    #[must_use]
    pub fn frame_size(&self) -> (u32, u32) {
        (
            self.rect.width().div_ceil(2),
            self.rect.height().div_ceil(2),
        )
    }

    /// Puts a frame of premultiplied RGBA on screen, enlarged to fill the
    /// overlay, and shows the overlay.
    ///
    /// # Errors
    /// Returns [`WinError`] if the frame has the wrong size or the update
    /// fails.
    pub fn present(&mut self, rgba: &[u8]) -> Result<(), WinError> {
        let (w, h) = self.frame_size();
        if rgba.len() != w as usize * h as usize * 4 {
            return Err(WinError::invalid("UpdateLayeredWindow", "overlay frame"));
        }
        let (full_w, _) = self.surface.size();
        double_into(rgba, w as usize, self.surface.pixels_mut(), full_w as usize);
        let now = Presented {
            dst: POINT {
                x: self.rect.left,
                y: self.rect.top,
            },
            size: SIZE {
                cx: self.rect.right - self.rect.left,
                cy: self.rect.bottom - self.rect.top,
            },
            opacity: 255,
            shift: 0,
        };
        update_layered(self.hwnd.raw(), self.surface.dc(), &now, None)?;
        if !self.shown {
            self.shown = true;
            // SAFETY: our window, on its own thread.
            let _ = unsafe { ShowWindow(self.hwnd.raw(), SW_SHOWNOACTIVATE) };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presents_frames_of_half_its_size() {
        // One pixel, so nothing is seen while tests run.
        let rect = RectPx {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        };
        let mut overlay = Overlay::create(rect).unwrap();
        assert_eq!(overlay.frame_size(), (1, 1));
        overlay.present(&[0; 4]).unwrap();
        assert!(overlay.present(&[0; 8]).is_err());
    }
}
