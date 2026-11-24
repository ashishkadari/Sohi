//! A desktop that only notes what was done to it.

use crate::Desktop;
use crate::apps::App;

pub(crate) struct FakeDesktop {
    pub did: Vec<String>,
    pub clipboard: Option<String>,
}

impl FakeDesktop {
    pub fn new() -> Self {
        Self {
            did: Vec::new(),
            clipboard: None,
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
}
