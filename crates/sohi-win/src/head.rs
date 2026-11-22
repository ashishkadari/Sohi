//! The head widget window (spec §4.2).
//!
//! A layered, topmost tool window that never activates. Its pixels come
//! from `UpdateLayeredWindow` (see `present`), so alpha-0 pixels are
//! hit-test transparent and clicks fall through to the window underneath.
//! While parked the window is hidden, so the compositor does no work for it.

use std::cell::Cell;

use sohi_core::geometry::RectPx;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_EXCLUDED_FROM_PEEK, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, ClientToScreen};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, IsWindowVisible, MA_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow, ULW_ALPHA, UpdateLayeredWindow,
    WM_DPICHANGED, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_RBUTTONUP, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::hwnd::{OwnedHwnd, WindowSpec, WndHandler, create_window, signed_words};
use crate::surface::{Surface, swizzle_rgba_to_bgra};
use crate::{EventQueue, WinError, WinEvent};

/// `WM_MOUSELEAVE` (0x02A3). The `windows` crate defines it under
/// `Win32_UI_Controls`; one constant is not worth that whole feature.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Window class of the head widget.
const HEAD_CLASS: &str = "Sohi.Head";

/// One frame of head pixels: `width * height` RGBA pixels with
/// premultiplied alpha (tiny-skia's `Pixmap` layout).
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major RGBA bytes, `width * height * 4` long.
    pub rgba: &'a [u8],
}

/// The head widget window. Created hidden.
pub struct HeadWindow {
    hwnd: OwnedHwnd,
    /// Lazily allocated on the first `present`, resized on DPI change.
    surface: Option<Surface>,
}

impl HeadWindow {
    /// Creates the (hidden) head window; mouse events go to `queue`.
    ///
    /// # Errors
    /// Returns [`WinError`] if window creation fails.
    pub fn create(queue: EventQueue) -> Result<Self, WinError> {
        let spec = WindowSpec {
            class_name: HEAD_CLASS,
            ex_style: WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            style: WS_POPUP,
        };
        let handler = HeadHandler {
            queue,
            tracking: Cell::new(false),
        };
        let hwnd = create_window(&spec, Box::new(handler))?;
        let excluded = TRUE;
        // SAFETY: valid window; the attribute takes a BOOL of the given size.
        // Failure only means Aero Peek may fade the widget, so it is ignored.
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd.raw(),
                DWMWA_EXCLUDED_FROM_PEEK,
                (&raw const excluded).cast(),
                u32::try_from(size_of_val(&excluded)).unwrap_or(0),
            )
        };
        Ok(Self {
            hwnd,
            surface: None,
        })
    }

    /// Puts `frame` on screen with its top-left at (`x`, `y`), multiplied by
    /// `opacity` (255 = as drawn). Moves and resizes the window to match.
    ///
    /// # Errors
    /// Returns [`WinError`] if the frame size is wrong or the update fails.
    pub fn present(
        &mut self,
        frame: Frame<'_>,
        x: i32,
        y: i32,
        opacity: u8,
    ) -> Result<(), WinError> {
        let expected = frame.width as usize * frame.height as usize * 4;
        if frame.rgba.len() != expected || frame.width == 0 || frame.height == 0 {
            return Err(WinError::invalid("UpdateLayeredWindow", "frame size"));
        }
        if self.surface.as_ref().map(Surface::size) != Some((frame.width, frame.height)) {
            // Drop the old DIB before allocating the new one.
            self.surface = None;
            self.surface = Some(Surface::new(frame.width, frame.height)?);
        }
        let Some(surface) = self.surface.as_mut() else {
            return Err(WinError::invalid("UpdateLayeredWindow", "surface"));
        };
        swizzle_rgba_to_bgra(frame.rgba, surface.pixels_mut());
        let dst = POINT { x, y };
        let size = SIZE {
            cx: i32::try_from(frame.width).unwrap_or(0),
            cy: i32::try_from(frame.height).unwrap_or(0),
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: u8::try_from(AC_SRC_OVER).unwrap_or(0),
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: u8::try_from(AC_SRC_ALPHA).unwrap_or(1),
        };
        // SAFETY: valid window and memory DC with the DIB selected; all
        // pointers refer to locals that outlive the call. A null screen DC
        // makes the system use the default palette.
        unsafe {
            UpdateLayeredWindow(
                self.hwnd.raw(),
                None,
                Some(&raw const dst),
                Some(&raw const size),
                Some(surface.dc()),
                Some(&raw const src),
                COLORREF(0),
                Some(&raw const blend),
                ULW_ALPHA,
            )
        }
        .map_err(|e| WinError::api("UpdateLayeredWindow", &e))
    }

    /// Shows the window without activating it and re-asserts topmost.
    pub fn show(&self) {
        // SAFETY: valid window owned by this thread.
        unsafe {
            let _ = ShowWindow(self.hwnd.raw(), SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                self.hwnd.raw(),
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Hides the window (parked).
    pub fn hide(&self) {
        // SAFETY: valid window owned by this thread.
        let _ = unsafe { ShowWindow(self.hwnd.raw(), SW_HIDE) };
    }

    /// Whether the window is currently shown.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        // SAFETY: valid window owned by this thread.
        unsafe { IsWindowVisible(self.hwnd.raw()) }.as_bool()
    }

    /// Moves and resizes the window without activating it.
    pub fn set_rect(&self, rect: RectPx) {
        // SAFETY: valid window owned by this thread.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd.raw(),
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                i32::try_from(rect.width()).unwrap_or(0),
                i32::try_from(rect.height()).unwrap_or(0),
                SWP_NOACTIVATE,
            )
        };
    }

    #[cfg(test)]
    pub(crate) fn raw(&self) -> HWND {
        self.hwnd.raw()
    }
}

struct HeadHandler {
    queue: EventQueue,
    /// Whether `TrackMouseEvent(TME_LEAVE)` is armed (pointer is over us).
    tracking: Cell<bool>,
}

impl HeadHandler {
    fn on_mouse_move(&self, hwnd: HWND) {
        if self.tracking.get() {
            return;
        }
        let mut tme = TRACKMOUSEEVENT {
            cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or(0),
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        // SAFETY: `tme` is fully initialized for a window on this thread.
        if unsafe { TrackMouseEvent(&raw mut tme) }.is_ok() {
            self.tracking.set(true);
            self.queue.push(WinEvent::HeadHover(true));
        }
    }
}

/// Mouse-message `LPARAM` to a client-coordinate point.
fn client_point(lparam: LPARAM) -> POINT {
    let (x, y) = signed_words(lparam.0);
    POINT { x, y }
}

impl WndHandler for HeadHandler {
    fn on_message(&self, hwnd: HWND, msg: u32, _wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_DPICHANGED => {
                // We size and place ourselves from the primary monitor, so
                // the suggested rectangle in lparam is ignored.
                self.queue.push(WinEvent::DisplayChanged);
                Some(LRESULT(0))
            }
            WM_MOUSEACTIVATE => Some(LRESULT(isize::try_from(MA_NOACTIVATE).unwrap_or(3))),
            WM_MOUSEMOVE => {
                self.on_mouse_move(hwnd);
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.tracking.set(false);
                self.queue.push(WinEvent::HeadHover(false));
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                self.queue.push(WinEvent::HeadClicked);
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                let mut pt = client_point(lparam);
                // SAFETY: valid window and point.
                let _ = unsafe { ClientToScreen(hwnd, &raw mut pt) };
                self.queue
                    .push(WinEvent::HeadContextMenu { x: pt.x, y: pt.y });
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW};

    #[test]
    fn head_window_has_no_activate_layered_topmost_tool_styles() {
        let head = HeadWindow::create(EventQueue::new()).unwrap();
        // SAFETY: valid window created on this thread.
        let ex = unsafe { GetWindowLongPtrW(head.raw(), GWL_EXSTYLE) };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ex = ex as u32;
        for flag in [
            WS_EX_LAYERED,
            WS_EX_TOPMOST,
            WS_EX_TOOLWINDOW,
            WS_EX_NOACTIVATE,
        ] {
            assert_eq!(ex & flag.0, flag.0, "missing {flag:?}");
        }
        assert!(!head.is_visible(), "created hidden");
    }

    #[test]
    fn lparam_coordinates_are_signed() {
        // x = -5, y = 300 packed as two 16-bit words.
        let packed = (300_isize << 16) | (0xFFFB_isize);
        let pt = client_point(LPARAM(packed));
        assert_eq!((pt.x, pt.y), (-5, 300));
    }
}
