//! A desktop that only notes what was done to it.

use crate::Desktop;
use crate::apps::App;

pub(crate) struct FakeDesktop {
    pub did: Vec<String>,
}

impl FakeDesktop {
    pub fn new() -> Self {
        Self { did: Vec::new() }
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
}
