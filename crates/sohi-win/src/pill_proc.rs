//! The text pill's window procedure and EDIT subclass procedure.

use std::cell::Cell;
use std::rc::Rc;

use sohi_core::geometry::RectPx;
use sohi_core::pill::PillLayout;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, HDC, PAINTSTRUCT, SetBkColor, SetBkMode, SetTextColor,
    TRANSPARENT, TextOutW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN, VK_UP};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    WA_INACTIVE, WM_ACTIVATE, WM_CHAR, WM_CLOSE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_ERASEBKGND,
    WM_KEYDOWN, WM_NCDESTROY, WM_PAINT,
};

use crate::gdi::{Brush, Font, rgb};
use crate::hwnd::WndHandler;
use crate::{EventQueue, WinEvent};

/// Spec §4.6 colours.
pub(crate) const BACKGROUND: u32 = 0x0014_1416;
pub(crate) const TEXT: u32 = 0xED_ED_ED;
/// The `›` prompt: the spec's GLOW red, readable on the dark background.
pub(crate) const PROMPT: u32 = 0xFF_4D_5E;
pub(crate) const SEPARATOR: u32 = 0x2A_2A_2E;
/// The accent at low intensity, for the DWM border.
pub(crate) const BORDER: u32 = 0x5A_0F_1A;

/// GDI objects shared with the window procedure.
pub(crate) struct Style {
    pub(crate) background: Brush,
    pub(crate) separator: Brush,
    pub(crate) font: Cell<Option<Font>>,
    pub(crate) line_px: Cell<i32>,
    pub(crate) layout: Cell<PillLayout>,
}

/// The pill's own window procedure: colours, painting and activation.
pub(crate) struct PillHandler {
    pub(crate) style: Rc<Style>,
    pub(crate) queue: EventQueue,
}

impl PillHandler {
    fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let l = self.style.layout.get();
        // SAFETY: BeginPaint/EndPaint pair on our window; the brushes and
        // font are alive in `self.style`.
        unsafe {
            let dc = BeginPaint(hwnd, &raw mut ps);
            let all = RECT {
                left: 0,
                top: 0,
                right: l.width,
                bottom: l.height,
            };
            FillRect(dc, &raw const all, self.style.background.raw());
            if let Some(sep) = l.separator {
                let sep = rect(sep);
                FillRect(dc, &raw const sep, self.style.separator.raw());
            }
            if let Some(font) = self.style.font.take() {
                let old = font.select(dc);
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, rgb(PROMPT));
                let prompt: Vec<u16> = "›".encode_utf16().collect();
                let _ = TextOutW(dc, l.prompt_x, l.input.top, &prompt);
                Font::restore(dc, old);
                self.style.font.set(Some(font));
            }
            let _ = EndPaint(hwnd, &raw const ps);
        }
    }

    fn colour(&self, dc: HDC) -> LRESULT {
        // SAFETY: `dc` is the control's DC, valid during this message.
        unsafe {
            SetTextColor(dc, rgb(TEXT));
            SetBkColor(dc, rgb(BACKGROUND));
        }
        LRESULT(self.style.background.raw().0 as isize)
    }
}

impl WndHandler for PillHandler {
    fn on_message(&self, hwnd: HWND, msg: u32, wparam: WPARAM, _lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => Some(self.colour(HDC(wparam.0 as *mut _))),
            WM_ERASEBKGND => Some(LRESULT(1)), // WM_PAINT fills everything
            WM_PAINT => {
                self.paint(hwnd);
                Some(LRESULT(0))
            }
            WM_ACTIVATE => {
                let active = (wparam.0 & 0xFFFF) != WA_INACTIVE as usize;
                self.queue.push(WinEvent::PillFocus(active));
                None
            }
            WM_CLOSE => {
                // Alt+F4 means "close", not "destroy": the app decides.
                self.queue.push(WinEvent::PillEscape);
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

fn rect(r: RectPx) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Subclass ids, which also tell the two controls apart.
pub(crate) const INPUT_ID: usize = 1;
pub(crate) const BUBBLE_ID: usize = 2;
pub(crate) const ENTER: u16 = VK_RETURN.0;
pub(crate) const ESC: u16 = VK_ESCAPE.0;
pub(crate) const UP: u16 = VK_UP.0;

/// Subclass procedure for both EDIT controls. In the input, Enter and ↑
/// become events and the beep a single-line edit makes for Enter/Esc is
/// swallowed; Esc closes from either control.
pub(crate) unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: `data` is the `EventQueue` box set in `PillWindow::create`,
    // alive until after WM_NCDESTROY.
    let queue = unsafe { &*(data as *const EventQueue) };
    let key = u16::try_from(wparam.0).unwrap_or(0);
    let input = id == INPUT_ID;
    let event = match (msg, key) {
        (WM_KEYDOWN, ENTER) if input => Some(WinEvent::PillSubmit),
        (WM_KEYDOWN, UP) if input => Some(WinEvent::PillRecall),
        (WM_KEYDOWN, ESC) => Some(WinEvent::PillEscape),
        (WM_CHAR, 0x0D | 0x1B) if input => return LRESULT(0),
        (WM_NCDESTROY, _) => {
            // SAFETY: removing our own subclass from our own window.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(edit_proc), id) };
            None
        }
        _ => None,
    };
    if let Some(event) = event {
        queue.push(event);
        return LRESULT(0);
    }
    // SAFETY: forwarding unmodified arguments.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
