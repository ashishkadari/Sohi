//! The two global hotkeys from `[wake]` (spec §4.7). A combination that
//! cannot be parsed or registered is logged and reported once with a tray
//! balloon; Sohi keeps running with whatever is left (at least the corner).

use sohi_core::config::WakeConfig;
use sohi_core::hotkey::Hotkey;
use sohi_win::{AppWindow, HotkeyRegistration, TrayIcon};

/// Toggles voice mode.
pub const VOICE: u16 = 1;
/// Opens text mode.
pub const TEXT: u16 = 2;

/// The registered hotkeys; each is unregistered on drop.
pub struct Hotkeys {
    _registrations: Vec<HotkeyRegistration>,
}

impl Hotkeys {
    /// Registers `hotkey_voice` and `hotkey_text`. Failures are logged and
    /// shown as one warning balloon on `tray`.
    pub fn register(window: &AppWindow, tray: &TrayIcon, wake: &WakeConfig) -> Self {
        let mut registrations = Vec::with_capacity(2);
        let mut failed = Vec::new();
        for (id, setting, text) in [
            (VOICE, "hotkey_voice", &wake.hotkey_voice),
            (TEXT, "hotkey_text", &wake.hotkey_text),
        ] {
            match register_one(window, id, text) {
                Ok((registration, hotkey)) => {
                    log::info!("hotkey {setting}: {hotkey}");
                    registrations.push(registration);
                }
                Err(reason) => {
                    log::warn!("hotkey {setting} = \"{text}\" unavailable: {reason}");
                    failed.push(text.as_str());
                }
            }
        }
        if !failed.is_empty() {
            let message = format!(
                "{} could not be set up (in use or invalid). The corner still works; \
                 change [wake] in config.toml.",
                failed.join(" and ")
            );
            if let Err(err) = tray.warn("Sohi hotkey unavailable", &message) {
                log::warn!("cannot show hotkey balloon: {err}");
            }
        }
        Self {
            _registrations: registrations,
        }
    }
}

fn register_one(
    window: &AppWindow,
    id: u16,
    text: &str,
) -> Result<(HotkeyRegistration, Hotkey), String> {
    let hotkey = Hotkey::parse(text).map_err(|e| e.to_string())?;
    let registration = HotkeyRegistration::register(window, id, &hotkey).map_err(|e| {
        log::debug!("RegisterHotKey failed: {e}");
        "already in use by another app".to_owned()
    })?;
    Ok((registration, hotkey))
}
