//! What the tools do to the PC, behind a trait so tests never touch it.

use std::path::Path;

use sohi_win::{MediaKey, WinError};

use crate::MediaAction;
use crate::apps::{App, start_menu};

/// The PC the tools act on. Errors are short texts for the model and the
/// user.
pub trait Desktop: Send {
    /// The installed apps, the preferred ones first.
    fn apps(&mut self) -> Vec<App>;

    /// Opens an app's target, or a web address that was validated.
    ///
    /// # Errors
    /// Returns why it could not be opened.
    fn open(&mut self, target: &str) -> Result<(), String>;

    /// The clipboard's text, at most `max_bytes` of it; `None` if it
    /// holds none.
    ///
    /// # Errors
    /// Returns why it could not be read.
    fn clipboard(&mut self, max_bytes: usize) -> Result<Option<String>, String>;

    /// Replaces the clipboard's contents.
    ///
    /// # Errors
    /// Returns why it could not be written.
    fn set_clipboard(&mut self, text: &str) -> Result<(), String>;

    /// Presses a media key.
    ///
    /// # Errors
    /// Returns why it could not be pressed.
    fn press(&mut self, key: MediaAction) -> Result<(), String>;

    /// The volume in percent.
    ///
    /// # Errors
    /// Returns why it is not known.
    fn volume(&mut self) -> Result<u8, String>;

    /// Sets the volume in percent.
    ///
    /// # Errors
    /// Returns why it could not be set.
    fn set_volume(&mut self, percent: u8) -> Result<(), String>;
}

/// This PC.
#[derive(Debug, Default)]
pub struct WinDesktop;

impl Desktop for WinDesktop {
    fn apps(&mut self) -> Vec<App> {
        let mut apps = start_menu();
        match sohi_win::apps_folder() {
            Ok(found) => apps.extend(found.into_iter().map(|(name, target)| App {
                name,
                target,
                exact_only: false,
            })),
            Err(err) => log::warn!("cannot list the apps folder: {err}"),
        }
        apps.extend(sohi_win::app_paths().into_iter().map(|name| App {
            target: name.clone(),
            name,
            exact_only: true,
        }));
        apps
    }

    fn open(&mut self, target: &str) -> Result<(), String> {
        sohi_win::shell::open_path(Path::new(target)).map_err(|err| {
            log::warn!("cannot open: {err:?}");
            "Windows could not open it".to_owned()
        })
    }

    fn clipboard(&mut self, max_bytes: usize) -> Result<Option<String>, String> {
        sohi_win::clipboard_text(max_bytes).map_err(|err| {
            log::warn!("cannot read the clipboard: {err}");
            "the clipboard is in use".to_owned()
        })
    }

    fn set_clipboard(&mut self, text: &str) -> Result<(), String> {
        sohi_win::set_clipboard_text(text).map_err(|err| {
            log::warn!("cannot write the clipboard: {err}");
            "the clipboard is in use".to_owned()
        })
    }

    fn press(&mut self, key: MediaAction) -> Result<(), String> {
        let key = match key {
            MediaAction::PlayPause => MediaKey::PlayPause,
            MediaAction::Next => MediaKey::Next,
            MediaAction::Previous => MediaKey::Previous,
            MediaAction::Mute => MediaKey::Mute,
        };
        sohi_win::press_media_key(key).map_err(|err| {
            log::warn!("cannot press a media key: {err}");
            "Windows refused the key".to_owned()
        })
    }

    fn volume(&mut self) -> Result<u8, String> {
        sohi_win::volume().map_err(|err| no_speakers(&err))
    }

    fn set_volume(&mut self, percent: u8) -> Result<(), String> {
        sohi_win::set_volume(percent).map_err(|err| no_speakers(&err))
    }
}

fn no_speakers(err: &WinError) -> String {
    log::warn!("cannot use the volume: {err}");
    "there are no speakers to control".to_owned()
}
