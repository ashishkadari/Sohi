//! Global hotkeys (spec §4.7): `RegisterHotKey` on the app window, with
//! `MOD_NOREPEAT` so holding the keys fires once. `WM_HOTKEY` becomes
//! [`WinEvent::Hotkey`](crate::WinEvent::Hotkey).

use sohi_core::hotkey::Hotkey;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VK_ESCAPE,
};

use crate::{AppWindow, WinError};

/// A registered hotkey, unregistered on drop.
#[derive(Debug)]
pub struct HotkeyRegistration {
    hwnd: HWND,
    id: i32,
}

impl HotkeyRegistration {
    /// Registers `hotkey` for `window` under `id` (`0..=0xBFFF`); presses
    /// arrive as `WinEvent::Hotkey { id }`.
    ///
    /// # Errors
    /// Returns [`WinError`] if the combination is already taken by another
    /// app (or this one) or reserved by Windows.
    pub fn register(window: &AppWindow, id: u16, hotkey: &Hotkey) -> Result<Self, WinError> {
        let hwnd = window.raw();
        let id = i32::from(id);
        // SAFETY: our window on this thread; the id is unique per window.
        unsafe { RegisterHotKey(Some(hwnd), id, modifiers(*hotkey), hotkey.vk) }
            .map_err(|e| WinError::api("RegisterHotKey", &e))?;
        Ok(Self { hwnd, id })
    }
}

/// Something that can own hotkeys registered later. Copy it freely; it is
/// only valid on the UI thread while the app window exists.
#[derive(Debug, Clone, Copy)]
pub struct HotkeyHost(HWND);

impl HotkeyHost {
    /// The host for `window`.
    #[must_use]
    pub fn new(window: &AppWindow) -> Self {
        Self(window.raw())
    }

    /// Takes plain Esc system-wide until dropped; presses arrive as
    /// `WinEvent::Hotkey { id }` and reach no other app.
    ///
    /// # Errors
    /// Returns [`WinError`] if another app already holds Esc.
    pub fn escape(self, id: u16) -> Result<HotkeyRegistration, WinError> {
        let id = i32::from(id);
        // SAFETY: the app window lives for the whole message loop and
        // belongs to this thread; the id is unique per window.
        unsafe { RegisterHotKey(Some(self.0), id, MOD_NOREPEAT, u32::from(VK_ESCAPE.0)) }
            .map_err(|e| WinError::api("RegisterHotKey", &e))?;
        Ok(HotkeyRegistration { hwnd: self.0, id })
    }
}

impl Drop for HotkeyRegistration {
    fn drop(&mut self) {
        // SAFETY: registered by us for this window and id; failure (window
        // already destroyed, which also frees it) is harmless.
        let _ = unsafe { UnregisterHotKey(Some(self.hwnd), self.id) };
    }
}

fn modifiers(hotkey: Hotkey) -> HOT_KEY_MODIFIERS {
    let mut mods = MOD_NOREPEAT;
    for (on, flag) in [
        (hotkey.ctrl, MOD_CONTROL),
        (hotkey.alt, MOD_ALT),
        (hotkey.shift, MOD_SHIFT),
        (hotkey.win, MOD_WIN),
    ] {
        if on {
            mods |= flag;
        }
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventQueue;

    #[test]
    fn modifiers_always_include_norepeat() {
        let hotkey = Hotkey::parse("Ctrl+Alt+Space").unwrap();
        assert_eq!(modifiers(hotkey), MOD_NOREPEAT | MOD_CONTROL | MOD_ALT);
    }

    #[test]
    fn a_taken_combination_fails_and_drop_frees_it() {
        let window = AppWindow::create("Sohi.Test.Hotkey", 0xC125, EventQueue::new()).unwrap();
        // An unlikely combination so the test does not clash with the
        // user's own shortcuts.
        let hotkey = Hotkey::parse("Ctrl+Alt+Shift+F23").unwrap();
        let first = HotkeyRegistration::register(&window, 1, &hotkey).unwrap();
        assert!(HotkeyRegistration::register(&window, 2, &hotkey).is_err());
        drop(first);
        let again = HotkeyRegistration::register(&window, 3, &hotkey);
        assert!(again.is_ok());
    }
}
