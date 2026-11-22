//! The text pill (spec §4.6): a dark popup under the head with a
//! single-line input and a read-only reply bubble.
//!
//! It is a normal (not layered) window, so the native EDIT controls give
//! IME, caret, selection, clipboard and accessibility for free. Keys the
//! app cares about (Enter, Esc, ↑) are caught by subclassing the input and
//! become [`WinEvent`]s; the text itself is fetched with
//! [`PillWindow::take_input`].

use std::cell::Cell;
use std::rc::Rc;

use sohi_core::geometry::RectPx;
use sohi_core::pill::{edit_text, pill_layout};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::UI::Controls::{
    EM_GETLINECOUNT, EM_REPLACESEL, EM_SCROLLCARET, EM_SETCUEBANNER, EM_SETSEL, SetWindowTheme,
    ShowScrollBar,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::SetWindowSubclass;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY,
    GetWindowTextLengthW, GetWindowTextW, HMENU, HWND_TOPMOST, IsWindowVisible, MoveWindow,
    SB_VERT, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SendMessageW, SetForegroundWindow,
    SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_SETFONT, WS_CHILD,
    WS_CLIPCHILDREN, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::gdi::{Brush, Font, rgb};
use crate::hwnd::{OwnedHwnd, WindowSpec, create_window, module_instance};
use crate::pill_proc::{
    BACKGROUND, BORDER, BUBBLE_ID, INPUT_ID, PillHandler, SEPARATOR, Style, edit_proc,
};
use crate::{EventQueue, WinError};

/// The pill window and its two EDIT controls.
pub struct PillWindow {
    // Dropped first: destroying the window destroys the children and runs
    // their WM_NCDESTROY (which removes the subclass) before `_keys` drops.
    hwnd: OwnedHwnd,
    input: HWND,
    bubble: HWND,
    style: Rc<Style>,
    /// Read by the subclass procedure through its reference data.
    _keys: Box<EventQueue>,
    origin: Cell<(i32, i32)>,
    dpi: Cell<u32>,
    lines: Cell<u32>,
}

impl PillWindow {
    /// Creates the (hidden) pill for monitor `dpi`. Key events go to `queue`.
    ///
    /// # Errors
    /// Returns [`WinError`] if a window or GDI object cannot be created.
    pub fn create(queue: EventQueue, dpi: u32) -> Result<Self, WinError> {
        let style = Rc::new(Style {
            background: Brush::solid(rgb(BACKGROUND))?,
            separator: Brush::solid(rgb(SEPARATOR))?,
            font: Cell::new(None),
            line_px: Cell::new(0),
            layout: Cell::new(pill_layout(dpi, 18, 0)),
        });
        let spec = WindowSpec {
            class_name: "Sohi.Pill",
            ex_style: WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            style: WS_POPUP | WS_CLIPCHILDREN,
        };
        let handler = PillHandler {
            style: Rc::clone(&style),
            queue: queue.clone(),
        };
        let hwnd = create_window(&spec, Box::new(handler))?;
        let input = child(hwnd.raw(), ES_AUTOHSCROLL, WS_VISIBLE)?;
        let bubble = child(
            hwnd.raw(),
            ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL,
            WS_VSCROLL,
        )?;
        let keys = Box::new(queue);
        for (edit, id) in [(input, INPUT_ID), (bubble, BUBBLE_ID)] {
            // SAFETY: `keys` is heap-allocated and outlives both children
            // (see the field order on `PillWindow`); the subclass is removed
            // on WM_NCDESTROY.
            unsafe {
                let _ = SetWindowSubclass(edit, Some(edit_proc), id, (&raw const *keys) as usize);
                let _ = SetWindowTheme(edit, w!("DarkMode_Explorer"), PCWSTR::null());
            }
        }
        decorate(hwnd.raw());
        let pill = Self {
            hwnd,
            input,
            bubble,
            style,
            _keys: keys,
            origin: Cell::new((0, 0)),
            dpi: Cell::new(dpi),
            lines: Cell::new(0),
        };
        pill.set_dpi(dpi)?;
        Ok(pill)
    }

    /// Recreates the font for a new monitor DPI and lays out again.
    ///
    /// # Errors
    /// Returns [`WinError`] if the font cannot be created.
    pub fn set_dpi(&self, dpi: u32) -> Result<(), WinError> {
        let font = Font::ui(dpi)?;
        let line_px = font.line_height(self.bubble);
        for edit in [self.input, self.bubble] {
            // SAFETY: `edit` is our child; the font outlives it (it is
            // replaced only after the controls switch to the new one).
            unsafe {
                SendMessageW(
                    edit,
                    WM_SETFONT,
                    Some(WPARAM(font.raw().0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
        self.style.font.set(Some(font));
        self.style.line_px.set(line_px);
        self.dpi.set(dpi);
        self.relayout();
        Ok(())
    }

    /// Moves the pill's top-left corner to screen point (`x`, `y`).
    pub fn set_origin(&self, x: i32, y: i32) {
        self.origin.set((x, y));
        self.relayout();
    }

    /// Shows the pill. With `focus` it becomes the foreground window and
    /// the caret goes to the input; without, it shows but does not
    /// activate (spec §4.6 focus rules).
    pub fn show(&self, focus: bool) {
        let hwnd = self.hwnd.raw();
        let l = self.style.layout.get();
        let (x, y) = self.origin.get();
        // SAFETY: our window, on its own thread.
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                l.width,
                l.height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if focus {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                let _ = SetFocus(Some(self.input));
            }
        }
    }

    /// Hides the pill.
    pub fn hide(&self) {
        // SAFETY: our window.
        unsafe {
            let _ = ShowWindow(self.hwnd.raw(), SW_HIDE);
        }
    }

    /// Whether the pill is on screen.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        // SAFETY: our window.
        unsafe { IsWindowVisible(self.hwnd.raw()) }.as_bool()
    }

    /// Sets the grey hint shown while the input is empty.
    pub fn set_placeholder(&self, text: &str) {
        let text = HSTRING::from(text);
        // SAFETY: EM_SETCUEBANNER copies the string; `text` outlives the call.
        unsafe {
            SendMessageW(
                self.input,
                EM_SETCUEBANNER,
                Some(WPARAM(1)), // also while focused
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
    }

    /// Returns the typed text and clears the input.
    #[must_use]
    pub fn take_input(&self) -> String {
        let text = window_text(self.input);
        set_text(self.input, "");
        text
    }

    /// Replaces the input with `text` (prompt recall) and puts the caret at
    /// its end.
    pub fn set_input(&self, text: &str) {
        set_text(self.input, text);
        // SAFETY: our child; -1/-1 selects nothing at the end.
        unsafe {
            SendMessageW(
                self.input,
                EM_SETSEL,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(-1)),
            );
        }
    }

    /// Replaces the bubble's text.
    pub fn set_bubble(&self, text: &str) {
        set_text(self.bubble, &edit_text(text));
        self.after_bubble_change();
    }

    /// Appends `text` to the bubble (streamed reply pieces).
    pub fn append_bubble(&self, text: &str) {
        let text = HSTRING::from(edit_text(text));
        // SAFETY: our child; EM_REPLACESEL copies the string.
        unsafe {
            let end = GetWindowTextLengthW(self.bubble);
            SendMessageW(
                self.bubble,
                EM_SETSEL,
                Some(WPARAM(usize::try_from(end).unwrap_or(0))),
                Some(LPARAM(isize::try_from(end).unwrap_or(0))),
            );
            SendMessageW(
                self.bubble,
                EM_REPLACESEL,
                Some(WPARAM(0)),
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
        self.after_bubble_change();
    }

    fn after_bubble_change(&self) {
        let empty = window_text_len(self.bubble) == 0;
        let lines = if empty {
            0
        } else {
            // SAFETY: our child.
            let n = unsafe { SendMessageW(self.bubble, EM_GETLINECOUNT, None, None) };
            u32::try_from(n.0).unwrap_or(1)
        };
        if lines != self.lines.get() {
            self.lines.set(lines);
            self.relayout();
        }
        // SAFETY: our child.
        unsafe {
            SendMessageW(self.bubble, EM_SCROLLCARET, None, None);
        }
    }

    fn relayout(&self) {
        let (dpi, line_px, lines) = (self.dpi.get(), self.style.line_px.get(), self.lines.get());
        let l = pill_layout(dpi, line_px, lines);
        self.style.layout.set(l);
        // Even while hidden the bubble keeps its real width, so its line
        // count (which depends on wrapping) is right for the next text.
        let bubble = l
            .bubble
            .or(pill_layout(dpi, line_px, 1).bubble)
            .unwrap_or_default();
        let (x, y) = self.origin.get();
        // SAFETY: our window and children, on their thread.
        unsafe {
            let _ = MoveWindow(self.hwnd.raw(), x, y, l.width, l.height, true);
            place(self.input, l.input);
            place(self.bubble, bubble);
            let _ = ShowScrollBar(self.bubble, SB_VERT, l.scrolls);
            let _ = ShowWindow(
                self.bubble,
                if l.bubble.is_some() { SW_SHOW } else { SW_HIDE },
            );
        }
    }
}

/// Creates an EDIT child.
fn child(parent: HWND, edit_style: i32, extra: WINDOW_STYLE) -> Result<HWND, WinError> {
    let style = WS_CHILD | extra | WINDOW_STYLE(edit_style.cast_unsigned());
    // SAFETY: `parent` is a live window on this thread; "EDIT" is a system
    // class.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            PCWSTR::null(),
            style,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU::default()),
            Some(module_instance()?),
            None,
        )
    }
    .map_err(|e| WinError::api("CreateWindowExW(EDIT)", &e))
}

/// Rounded corners, a dim accent border and dark mode (Windows 11).
fn decorate(hwnd: HWND) {
    let corner = DWMWCP_ROUND;
    let border = rgb(BORDER);
    let dark: u32 = 1;
    // SAFETY: each pointer and size describes a local of the matching type.
    // Older Windows versions reject unknown attributes harmlessly.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corner).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&raw const border).cast(), 4);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            4,
        );
    }
}

/// # Safety
/// `hwnd` must be a live window on this thread.
unsafe fn place(hwnd: HWND, r: RectPx) {
    // SAFETY: per the function contract.
    let _ = unsafe {
        MoveWindow(
            hwnd,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            true,
        )
    };
}

fn window_text_len(hwnd: HWND) -> usize {
    // SAFETY: any window handle is accepted.
    usize::try_from(unsafe { GetWindowTextLengthW(hwnd) }).unwrap_or(0)
}

fn window_text(hwnd: HWND) -> String {
    let mut buffer = vec![0u16; window_text_len(hwnd) + 1];
    // SAFETY: `buffer` has room for the text and its terminator.
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(copied).unwrap_or(0)])
}

fn set_text(hwnd: HWND, text: &str) {
    // SAFETY: the window copies the string.
    let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(text)) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WinEvent;
    use crate::pill_proc::{ENTER, ESC, UP};
    use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

    fn key(hwnd: HWND, vk: u16) {
        // SAFETY: in-process send to our own child window.
        unsafe {
            SendMessageW(
                hwnd,
                WM_KEYDOWN,
                Some(WPARAM(usize::from(vk))),
                Some(LPARAM(0)),
            );
        }
    }

    /// Queued events, minus focus changes (which depend on what else is
    /// on screen while tests run).
    fn drain(queue: &EventQueue) -> Vec<WinEvent> {
        std::iter::from_fn(|| queue.pop())
            .filter(|e| !matches!(e, WinEvent::PillFocus(_)))
            .collect()
    }

    #[test]
    fn keys_become_events_only_where_they_apply() {
        let queue = EventQueue::new();
        let pill = PillWindow::create(queue.clone(), 96).unwrap();
        key(pill.input, ENTER);
        key(pill.input, UP);
        key(pill.input, ESC);
        key(pill.bubble, ENTER); // read-only bubble: nothing to submit
        key(pill.bubble, UP); // scrolls the bubble, no recall
        key(pill.bubble, ESC);
        assert_eq!(
            drain(&queue),
            [
                WinEvent::PillSubmit,
                WinEvent::PillRecall,
                WinEvent::PillEscape,
                WinEvent::PillEscape,
            ]
        );
    }

    #[test]
    fn input_round_trips_and_clears() {
        let pill = PillWindow::create(EventQueue::new(), 96).unwrap();
        pill.set_input("what time is it? 日本語");
        assert_eq!(pill.take_input(), "what time is it? 日本語");
        assert_eq!(pill.take_input(), "");
    }

    #[test]
    fn the_bubble_grows_then_scrolls() {
        let pill = PillWindow::create(EventQueue::new(), 96).unwrap();
        let row = pill.style.layout.get().height;
        assert_eq!(pill.lines.get(), 0);

        pill.set_bubble("› hi\nHey, Ashish.");
        assert_eq!(pill.lines.get(), 2);
        let two = pill.style.layout.get();
        assert!(two.height > row && !two.scrolls);

        // Streamed pieces, including a \r\n split across two of them.
        pill.append_bubble("\r");
        pill.append_bubble("\nline three\nfour\nfive\nsix\nseven");
        assert_eq!(pill.lines.get(), 7);
        assert!(pill.style.layout.get().scrolls);

        pill.set_bubble("");
        assert_eq!(pill.lines.get(), 0);
        assert_eq!(pill.style.layout.get().height, row);
    }

    /// Shows a pill for a few seconds so it can be captured on screen.
    #[test]
    #[ignore = "puts a window on screen"]
    fn show_on_screen() {
        let pill = PillWindow::create(EventQueue::new(), 144).unwrap();
        pill.set_origin(400, 100);
        pill.set_placeholder("Type a message…");
        pill.set_bubble("› what's the weather like?\nNo idea, Ashish. I can't see outside yet. Give me a network and a reason.");
        pill.show(false);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while std::time::Instant::now() < end {
            crate::msgloop::pump_for_tests();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
}
