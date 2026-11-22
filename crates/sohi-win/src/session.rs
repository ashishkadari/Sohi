//! Session lock, display power and suspend notifications (spec §4.7):
//! corner polling stops while nobody can see or use the desktop.
//!
//! [`PresenceWatch`] registers the app window for `WM_WTSSESSION_CHANGE`
//! and the console display power setting; `WM_POWERBROADCAST` suspend and
//! resume arrive without registration. The window turns them into
//! [`WinEvent::Presence`](crate::WinEvent::Presence).

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Power::{
    HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
    UnregisterPowerSettingNotification,
};
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_WINDOW_HANDLE, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
    PBT_POWERSETTINGCHANGE, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use windows::core::GUID;

use crate::{AppWindow, WinError};

/// `GUID_CONSOLE_DISPLAY_STATE` (data: 0 off, 1 on, 2 dimmed). Spelled out
/// to avoid the whole `Win32_System_SystemServices` feature for one GUID.
const GUID_CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe6_9556_704a_47a0_8f24_c28d_936f_da47);

/// Something that hides the desktop from the user, or brings it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The session was locked (`true`) or unlocked (`false`).
    Locked(bool),
    /// The console display turned off (`true`) or on/dimmed (`false`).
    DisplayOff(bool),
    /// The system is suspending (`true`) or resumed (`false`).
    Suspended(bool),
}

/// Lock and display-power registrations for the app window; undone on
/// drop.
#[derive(Debug)]
pub struct PresenceWatch {
    hwnd: HWND,
    session: bool,
    display: Option<HPOWERNOTIFY>,
}

impl PresenceWatch {
    /// Registers `window`. Each registration that fails is skipped, so
    /// polling just keeps running through that kind of absence.
    ///
    /// # Errors
    /// Returns [`WinError`] only if *both* registrations fail.
    pub fn register(window: &AppWindow) -> Result<Self, WinError> {
        let hwnd = window.raw();
        // SAFETY: our window on this thread; undone in Drop.
        let session = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) };
        let guid = GUID_CONSOLE_DISPLAY_STATE;
        // SAFETY: a window handle passed as HANDLE with
        // DEVICE_NOTIFY_WINDOW_HANDLE; `guid` outlives the call.
        let display = unsafe {
            RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &raw const guid,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
        };
        match (session, display) {
            (Err(e), Err(_)) => Err(WinError::api("WTSRegisterSessionNotification", &e)),
            (session, display) => Ok(Self {
                hwnd,
                session: session.is_ok(),
                display: display.ok(),
            }),
        }
    }

    /// Whether lock/unlock notifications are registered.
    #[must_use]
    pub fn watches_lock(&self) -> bool {
        self.session
    }

    /// Whether display power notifications are registered.
    #[must_use]
    pub fn watches_display(&self) -> bool {
        self.display.is_some()
    }
}

impl Drop for PresenceWatch {
    fn drop(&mut self) {
        if self.session {
            // SAFETY: registered for this window in `register`.
            let _ = unsafe { WTSUnRegisterSessionNotification(self.hwnd) };
        }
        if let Some(handle) = self.display.take() {
            // SAFETY: the handle came from RegisterPowerSettingNotification.
            let _ = unsafe { UnregisterPowerSettingNotification(handle) };
        }
    }
}

/// Decodes `WM_WTSSESSION_CHANGE`.
pub(crate) fn session_change(wparam: WPARAM) -> Option<Presence> {
    match u32::try_from(wparam.0).ok()? {
        WTS_SESSION_LOCK => Some(Presence::Locked(true)),
        WTS_SESSION_UNLOCK => Some(Presence::Locked(false)),
        _ => None,
    }
}

/// Decodes `WM_POWERBROADCAST`.
pub(crate) fn power_broadcast(wparam: WPARAM, lparam: LPARAM) -> Option<Presence> {
    match u32::try_from(wparam.0).ok()? {
        PBT_APMSUSPEND => Some(Presence::Suspended(true)),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(Presence::Suspended(false)),
        PBT_POWERSETTINGCHANGE if lparam.0 != 0 => {
            // SAFETY: for PBT_POWERSETTINGCHANGE, lparam points to a
            // POWERBROADCAST_SETTING valid for the duration of the message,
            // with DataLength bytes of data starting at `Data`.
            let state = unsafe { display_state(lparam.0 as *const POWERBROADCAST_SETTING) }?;
            Some(Presence::DisplayOff(state == 0))
        }
        _ => None,
    }
}

/// Reads the DWORD display state from a console-display power setting.
///
/// # Safety
/// `setting` must point to a valid `POWERBROADCAST_SETTING` followed by
/// its `DataLength` data bytes.
unsafe fn display_state(setting: *const POWERBROADCAST_SETTING) -> Option<u32> {
    // SAFETY: guaranteed by the caller.
    let header = unsafe { &*setting };
    if header.PowerSetting != GUID_CONSOLE_DISPLAY_STATE || header.DataLength < 4 {
        return None;
    }
    // SAFETY: at least 4 data bytes follow `Data`'s start; the read is
    // unaligned-safe.
    Some(unsafe { std::ptr::read_unaligned((&raw const header.Data).cast::<u32>()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventQueue;

    #[test]
    fn lock_and_power_messages_decode() {
        assert_eq!(
            session_change(WPARAM(WTS_SESSION_LOCK as usize)),
            Some(Presence::Locked(true))
        );
        assert_eq!(
            session_change(WPARAM(WTS_SESSION_UNLOCK as usize)),
            Some(Presence::Locked(false))
        );
        assert_eq!(session_change(WPARAM(1)), None, "console connect");
        assert_eq!(
            power_broadcast(WPARAM(PBT_APMSUSPEND as usize), LPARAM(0)),
            Some(Presence::Suspended(true))
        );
        assert_eq!(
            power_broadcast(WPARAM(PBT_APMRESUMEAUTOMATIC as usize), LPARAM(0)),
            Some(Presence::Suspended(false))
        );
    }

    #[repr(C)]
    struct Setting {
        guid: GUID,
        len: u32,
        data: u32,
    }

    #[test]
    fn display_state_reads_the_dword() {
        let change = |guid: GUID, data: u32| {
            let s = Setting { guid, len: 4, data };
            power_broadcast(
                WPARAM(PBT_POWERSETTINGCHANGE as usize),
                LPARAM((&raw const s) as isize),
            )
        };
        let display = GUID_CONSOLE_DISPLAY_STATE;
        assert_eq!(change(display, 0), Some(Presence::DisplayOff(true)));
        assert_eq!(change(display, 1), Some(Presence::DisplayOff(false)));
        assert_eq!(
            change(display, 2),
            Some(Presence::DisplayOff(false)),
            "dimmed"
        );
        assert_eq!(change(GUID::from_u128(1), 0), None, "other setting");
    }

    #[test]
    fn registration_succeeds_on_the_desktop() {
        let window = AppWindow::create("Sohi.Test.Presence", 0xC126, EventQueue::new()).unwrap();
        let watch = PresenceWatch::register(&window).unwrap();
        assert!(watch.watches_lock() || watch.watches_display());
    }
}
