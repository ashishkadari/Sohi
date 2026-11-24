//! What the tools do to the PC, behind a trait so tests never touch it.

use std::path::Path;

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
}
