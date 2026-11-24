//! What the tools do to the PC, behind a trait so tests never touch it.

use sohi_win::{MediaCommand, MediaKey, MediaResult, WinError};

use crate::apps::{App, start_menu};
use crate::{MediaAction, helper};

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

    /// Controls what is playing.
    ///
    /// # Errors
    /// Returns why that was not possible.
    fn media(&mut self, action: MediaAction) -> Result<MediaResult, String>;

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
        let found = helper::list().map_or_else(sohi_win::apps_folder, Ok);
        match found {
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
        let opened = helper::open(target)
            .inspect_err(|err| log::warn!("cannot start the helper: {err}"))
            .unwrap_or(false);
        if opened {
            Ok(())
        } else {
            Err("Windows could not open it".to_owned())
        }
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

    fn media(&mut self, action: MediaAction) -> Result<MediaResult, String> {
        let command = match action {
            MediaAction::Play => MediaCommand::Play,
            MediaAction::Pause => MediaCommand::Pause,
            MediaAction::Next => MediaCommand::Next,
            MediaAction::Previous => MediaCommand::Previous,
            MediaAction::Mute => return mute(),
        };
        let result = helper::media(command).map_err(|err| {
            log::warn!("cannot start the helper: {err}");
            "Windows' media controls did not work".to_owned()
        })?;
        log::info!("media {command:?}: {result:?}");
        result.ok_or_else(|| "Windows' media controls did not work".to_owned())
    }

    fn volume(&mut self) -> Result<u8, String> {
        sohi_win::volume().map_err(|err| no_speakers(&err))
    }

    fn set_volume(&mut self, percent: u8) -> Result<(), String> {
        sohi_win::set_volume(percent).map_err(|err| no_speakers(&err))
    }
}

fn mute() -> Result<MediaResult, String> {
    sohi_win::press_media_key(MediaKey::Mute)
        .map(|()| MediaResult::Done)
        .map_err(|err| {
            log::warn!("cannot press a media key: {err}");
            "Windows refused the key".to_owned()
        })
}

fn no_speakers(err: &WinError) -> String {
    log::warn!("cannot use the volume: {err}");
    "there are no speakers to control".to_owned()
}
