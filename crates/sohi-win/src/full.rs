//! The full mode window (spec §4.9): the conversation, an input and a few
//! buttons over the whole primary monitor.

use std::cell::Cell;
use std::rc::Rc;

use sohi_core::full::{BUTTONS, layout};
use sohi_core::geometry::RectPx;
use sohi_core::pill::edit_text;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{GetDC, InvalidateRect, ReleaseDC};
use windows::Win32::UI::Controls::{EM_SETSEL, SetWindowTheme};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::SetWindowSubclass;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_OWNERDRAW, CreateWindowExW, DestroyIcon, ES_AUTOVSCROLL, ES_MULTILINE, GetForegroundWindow,
    HICON, HMENU, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, ICON_BIG, ICON_SMALL, MoveWindow,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SendMessageW,
    SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_SETFONT, WM_SETICON, WS_CHILD, WS_CLIPCHILDREN, WS_EX_APPWINDOW, WS_POPUP, WS_TABSTOP,
    WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::full_proc::{
    FIRST_BUTTON, Fonts, FullHandler, INPUT_ID, Keys, Paint, TRANSCRIPT_ID, control_proc, rect,
};
use crate::gdi::Font;
use crate::hwnd::{OwnedHwnd, WindowSpec, create_window, module_instance};
use crate::pill::{window_text, window_text_len};
use crate::surface::Surface;
use crate::transcript::{GROUND, TextStyle, Transcript};
use crate::{EventQueue, WinError};

/// The window and its controls. Field order is drop order.
pub struct FullWindow {
    /// First: this destroys the controls, whose library and subclass data
    /// follow below.
    hwnd: OwnedHwnd,
    transcript: Transcript,
    input: HWND,
    buttons: [HWND; BUTTONS],
    paint: Rc<Paint>,
    /// Read by the subclass procedure through its reference data.
    _keys: Box<Keys>,
    icon: HICON,
}

impl FullWindow {
    /// Creates the (hidden) window over `bounds` for monitor `dpi`. Its
    /// events go to `queue`; `icon` draws its taskbar icon at a size.
    ///
    /// # Errors
    /// Returns [`WinError`] if a window or GDI object cannot be created.
    pub fn create(
        queue: &EventQueue,
        bounds: RectPx,
        dpi: u32,
        icon: impl Fn(u32) -> Vec<u8>,
    ) -> Result<Self, WinError> {
        let paint = Rc::new(Paint::new()?);
        let input_cell = Rc::new(Cell::new(HWND::default()));
        let spec = WindowSpec {
            class_name: "Sohi.Full",
            ex_style: WS_EX_APPWINDOW,
            style: WS_POPUP | WS_CLIPCHILDREN,
        };
        let handler = FullHandler {
            paint: Rc::clone(&paint),
            queue: queue.clone(),
            input: Rc::clone(&input_cell),
        };
        let hwnd = create_window(&spec, Box::new(handler))?;
        let transcript = Transcript::create(hwnd.raw())?;
        let input = child(hwnd.raw(), w!("EDIT"), ES_MULTILINE | ES_AUTOVSCROLL, 0)?;
        input_cell.set(input);
        let mut buttons = [HWND::default(); BUTTONS];
        for (i, (button, label)) in buttons.iter_mut().zip(paint.labels.take()).enumerate() {
            *button = child(hwnd.raw(), w!("BUTTON"), BS_OWNERDRAW, FIRST_BUTTON + i)?;
            paint.labels.borrow_mut()[i] = label;
            set_text(*button, label);
        }
        let keys = Box::new(Keys {
            queue: queue.clone(),
            input: input_cell,
        });
        for (control, id) in [(input, INPUT_ID), (transcript.raw(), TRANSCRIPT_ID)] {
            let data = (&raw const *keys) as usize;
            // SAFETY: `keys` is heap-allocated and outlives the controls
            // (see the field order); the subclass goes on WM_NCDESTROY.
            unsafe {
                let _ = SetWindowSubclass(control, Some(control_proc), id, data);
            }
        }
        // SAFETY: our control.
        let _ = unsafe { SetWindowTheme(input, w!("DarkMode_Explorer"), PCWSTR::null()) };
        let full = Self {
            icon: crate::tray::create_icon(dpi, &icon)?,
            hwnd,
            transcript,
            input,
            buttons,
            paint,
            _keys: keys,
        };
        full.name();
        full.set_bounds(bounds, dpi)?;
        Ok(full)
    }

    /// Title and icon, as the taskbar and Alt+Tab show them.
    fn name(&self) {
        set_text(self.hwnd.raw(), "Sohi");
        for size in [ICON_SMALL, ICON_BIG] {
            let icon = LPARAM(self.icon.0 as isize);
            // SAFETY: our window; the icon outlives it.
            unsafe {
                let size = WPARAM(size as usize);
                SendMessageW(self.hwnd.raw(), WM_SETICON, Some(size), Some(icon));
            }
        }
    }

    /// Covers `bounds`, laid out for `dpi`.
    ///
    /// # Errors
    /// Returns [`WinError`] if a font cannot be created.
    pub fn set_bounds(&self, bounds: RectPx, dpi: u32) -> Result<(), WinError> {
        let fonts = Fonts {
            input: Font::sized(dpi, 24, false)?,
            small: Font::sized(dpi, 20, false)?,
            title: Font::sized(dpi, 40, true)?,
        };
        let font = WPARAM(fonts.input.raw().0 as usize);
        // SAFETY: our control; the font is kept until the next one is set.
        unsafe { SendMessageW(self.input, WM_SETFONT, Some(font), Some(LPARAM(1))) };
        self.paint.fonts.replace(Some(fonts));
        let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
        let l = layout(width, height, dpi);
        self.paint.layout.set(l);
        self.paint.size.set((width, height));
        let children = [(self.transcript.raw(), l.transcript), (self.input, l.input)];
        let buttons = self.buttons.into_iter().zip(l.buttons);
        // SAFETY: our window and children, on their thread.
        unsafe {
            let hwnd = self.hwnd.raw();
            let _ = MoveWindow(hwnd, bounds.left, bounds.top, width, height, true);
            for (child, r) in children.into_iter().chain(buttons) {
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                let _ = MoveWindow(child, r.left, r.top, w, h, true);
            }
        }
        Ok(())
    }

    /// Shows the window in front of the others, under any top-most ones,
    /// without taking the keyboard.
    pub fn show(&self) {
        // Windows may refuse a background process the front, but never to
        // its top-most windows: so it is one for a moment.
        for after in [HWND_TOPMOST, HWND_NOTOPMOST, HWND_TOP] {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW;
            // SAFETY: our window.
            let _ = unsafe { SetWindowPos(self.hwnd.raw(), Some(after), 0, 0, 0, 0, flags) };
        }
    }

    /// Takes the keyboard, if Windows lets it. Returns whether it did.
    #[must_use]
    pub fn focus(&self) -> bool {
        // SAFETY: our window and child.
        unsafe {
            let _ = ShowWindow(self.hwnd.raw(), SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd.raw());
            let _ = SetFocus(Some(self.input));
            GetForegroundWindow() == self.hwnd.raw()
        }
    }

    /// Hides the window.
    pub fn hide(&self) {
        // SAFETY: our window.
        let _ = unsafe { ShowWindow(self.hwnd.raw(), SW_HIDE) };
    }

    /// Whether the window is in front.
    #[must_use]
    pub fn is_front(&self) -> bool {
        // SAFETY: no preconditions.
        unsafe { GetForegroundWindow() == self.hwnd.raw() }
    }

    /// Adds to the conversation.
    pub fn append(&self, text: &str, style: TextStyle) {
        self.transcript.append(text, style);
    }

    /// Empties the conversation.
    pub fn clear(&self) {
        self.transcript.clear();
    }

    /// Whether the input holds any text.
    #[must_use]
    pub fn has_input(&self) -> bool {
        window_text_len(self.input) > 0
    }

    /// Returns the typed text and clears the input.
    #[must_use]
    pub fn take_input(&self) -> String {
        let text = window_text(self.input);
        set_text(self.input, "");
        text.replace("\r\n", "\n")
    }

    /// Replaces the input with `text` and puts the caret at its end.
    pub fn set_input(&self, text: &str) {
        set_text(self.input, &edit_text(text));
        // SAFETY: our child; selects nothing at the end.
        unsafe {
            let end = Some(WPARAM(usize::MAX >> 1));
            SendMessageW(self.input, EM_SETSEL, end, Some(LPARAM(isize::MAX)));
        }
    }

    /// Says what Sohi is doing.
    pub fn set_status(&self, status: &str) {
        if self.paint.status.replace(status.to_owned()) != status {
            let r = rect(self.paint.layout.get().status);
            // SAFETY: our window.
            let _ = unsafe { InvalidateRect(Some(self.hwnd.raw()), Some(&raw const r), false) };
        }
    }

    /// Names the first button.
    pub fn set_talk_label(&self, label: &'static str) {
        let old = std::mem::replace(&mut self.paint.labels.borrow_mut()[0], label);
        if old != label {
            set_text(self.buttons[0], label);
            // SAFETY: our child.
            let _ = unsafe { InvalidateRect(Some(self.buttons[0]), None, false) };
        }
    }

    /// The size of the head in the header, in pixels.
    #[must_use]
    pub fn avatar_size(&self) -> (u32, u32) {
        let r = self.paint.layout.get().avatar;
        (r.width(), r.height())
    }

    /// Draws the head in the header: premultiplied RGBA of
    /// [`FullWindow::avatar_size`].
    ///
    /// # Errors
    /// Returns [`WinError`] if its bitmap cannot be created.
    pub fn set_avatar(&self, rgba: &[u8]) -> Result<(), WinError> {
        let (w, h) = self.avatar_size();
        let mut avatar = self.paint.avatar.borrow_mut();
        if avatar.as_ref().map(Surface::size) != Some((w, h)) {
            *avatar = Some(Surface::new(w, h)?);
        }
        if let Some(surface) = avatar.as_mut() {
            blend(rgba, surface.pixels_mut());
        }
        drop(avatar);
        // SAFETY: GetDC/ReleaseDC pair on our window.
        unsafe {
            let dc = GetDC(Some(self.hwnd.raw()));
            self.paint.draw_avatar(dc);
            ReleaseDC(Some(self.hwnd.raw()), dc);
        }
        Ok(())
    }
}

impl Drop for FullWindow {
    fn drop(&mut self) {
        // SAFETY: our own icon. The window may show it until it is
        // destroyed right after; Windows copes with a stale handle.
        let _ = unsafe { DestroyIcon(self.icon) };
    }
}

/// Puts premultiplied RGBA on the window's ground, as BGRA.
fn blend(rgba: &[u8], out: &mut [u8]) {
    let [_, r, g, b] = GROUND.to_be_bytes();
    for (&[sr, sg, sb, a], dst) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(out.as_chunks_mut::<4>().0)
    {
        let rest = 255 - u16::from(a);
        let mix = |s: u8, g: u8| {
            let under = u16::from(g) * rest / 255;
            u8::try_from(u16::from(s) + under).unwrap_or(255)
        };
        *dst = [mix(sb, b), mix(sg, g), mix(sr, r), 255];
    }
}

/// Creates a visible child control.
fn child(parent: HWND, class: PCWSTR, bits: i32, id: usize) -> Result<HWND, WinError> {
    let style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(bits.cast_unsigned());
    // SAFETY: `parent` is a live window on this thread; `class` is a system
    // class; for a child window the menu handle is its id.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            style,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut _)),
            Some(module_instance()?),
            None,
        )
    }
    .map_err(|e| WinError::api("CreateWindowExW(child)", &e))
}

fn set_text(hwnd: HWND, text: &str) {
    // SAFETY: the window copies the string.
    let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(text)) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FullEvent, WinEvent};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
    use windows::Win32::UI::WindowsAndMessaging::{WM_CLOSE, WM_COMMAND, WM_KEYDOWN};

    fn window() -> (EventQueue, FullWindow) {
        let queue = EventQueue::default();
        let bounds = RectPx {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1200,
        };
        let icon = |size: u32| vec![0; (size * size * 4) as usize];
        let full = FullWindow::create(&queue, bounds, 144, icon).unwrap();
        (queue, full)
    }

    fn send(hwnd: HWND, msg: u32, wparam: usize) {
        // SAFETY: in-process send to our own window.
        unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(0))) };
    }

    fn events(queue: &EventQueue) -> Vec<WinEvent> {
        std::iter::from_fn(|| queue.pop()).collect()
    }

    #[test]
    fn keys_buttons_and_closing_become_events() {
        let (queue, full) = window();
        send(full.input, WM_KEYDOWN, usize::from(VK_RETURN.0));
        send(full.input, WM_KEYDOWN, usize::from(VK_ESCAPE.0));
        send(full.hwnd.raw(), WM_COMMAND, FIRST_BUTTON + 1);
        send(full.hwnd.raw(), WM_CLOSE, 0);
        assert_eq!(
            events(&queue),
            [
                WinEvent::Full(FullEvent::Submit),
                WinEvent::Full(FullEvent::Escape),
                WinEvent::Full(FullEvent::CopyCode),
                WinEvent::Full(FullEvent::Exit),
            ]
        );
    }

    #[test]
    fn several_lines_can_be_typed() {
        let (_, full) = window();
        full.set_input("a\nb");
        assert!(full.has_input());
        assert_eq!(full.take_input(), "a\nb");
        assert!(!full.has_input());
    }

    #[test]
    fn the_avatar_is_put_on_the_ground() {
        let (_, full) = window();
        let (w, h) = full.avatar_size();
        full.set_avatar(&vec![0; (w * h * 4) as usize]).unwrap();
        let mut out = [0; 8];
        blend(&[0, 0, 0, 0, 10, 20, 30, 255], &mut out);
        assert_eq!(out, [0x0C, 0x0A, 0x0E, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn it_can_be_made_and_dropped_again() {
        for _ in 0..3 {
            let (_, full) = window();
            full.append("x", TextStyle::Code);
        }
    }
}
