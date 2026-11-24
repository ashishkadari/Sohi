//! A desktop that only notes what was done to it.

use crate::apps::App;
use sohi_win::MediaResult;

use crate::{Desktop, MediaAction};

pub(crate) struct FakeDesktop {
    pub did: Vec<String>,
    pub clipboard: Option<String>,
    pub volume: u8,
    pub media: MediaResult,
}

impl FakeDesktop {
    pub fn new() -> Self {
        Self {
            did: Vec::new(),
            clipboard: None,
            volume: 50,
            media: MediaResult::Done,
        }
    }
}

impl Desktop for FakeDesktop {
    fn apps(&mut self) -> Vec<App> {
        self.did.push("list apps".to_owned());
        vec![App {
            name: "Notepad".to_owned(),
            target: "notepad.lnk".to_owned(),
            exact_only: false,
        }]
    }

    fn open(&mut self, target: &str) -> Result<(), String> {
        self.did.push(format!("open {target}"));
        Ok(())
    }

    fn clipboard(&mut self, max_bytes: usize) -> Result<Option<String>, String> {
        self.did.push(format!("read {max_bytes}"));
        Ok(self.clipboard.clone())
    }

    fn set_clipboard(&mut self, text: &str) -> Result<(), String> {
        self.did.push(format!("copy {text}"));
        Ok(())
    }

    fn media(&mut self, action: MediaAction) -> Result<MediaResult, String> {
        self.did.push(format!("media {action:?}"));
        Ok(self.media)
    }

    fn volume(&mut self) -> Result<u8, String> {
        Ok(self.volume)
    }

    fn set_volume(&mut self, percent: u8) -> Result<(), String> {
        self.did.push(format!("volume {percent}"));
        self.volume = percent;
        Ok(())
    }
}
