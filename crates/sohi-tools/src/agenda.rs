//! The user's reminders and the file they are kept in (spec §5.10). The
//! tools change them from the runtime thread, the UI thread announces
//! them.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sohi_core::reminder::{Reminder, Reminders, Repeat, When};
use sohi_core::stamp::Stamp;

use crate::Outcome;

/// The reminders. Clones are handles to the same ones.
#[derive(Clone)]
pub struct Agenda {
    list: Arc<Mutex<Reminders>>,
    /// Where they are kept; `None` only in memory.
    file: Option<PathBuf>,
    now: fn() -> Stamp,
    changed: Arc<dyn Fn() + Send + Sync>,
}

impl fmt::Debug for Agenda {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agenda")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl Default for Agenda {
    /// An empty one that is not kept.
    fn default() -> Self {
        Self {
            list: Arc::default(),
            file: None,
            now: sohi_win::local_stamp,
            changed: Arc::new(|| {}),
        }
    }
}

impl Agenda {
    /// The reminders kept in `file`. `changed` is called whenever a tool
    /// changed them, on whatever thread that was.
    #[must_use]
    pub fn open(file: PathBuf, changed: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            list: Arc::new(Mutex::new(load(&file))),
            file: Some(file),
            changed: Arc::new(changed),
            ..Self::default()
        }
    }

    /// When the next one is due.
    #[must_use]
    pub fn next_at(&self) -> Option<Stamp> {
        self.lock().next_at()
    }

    /// Takes what is due at `now`.
    #[must_use]
    pub fn take_due(&self, now: Stamp) -> Vec<Reminder> {
        let mut list = self.lock();
        let due = list.take_due(now);
        // Announced again after a restart, at worst.
        if !due.is_empty()
            && let Err(err) = self.save(&list)
        {
            log::warn!("cannot save the reminders: {err}");
        }
        due
    }

    /// All of them, each on a line.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let now = (self.now)();
        self.lock().all().iter().map(|r| r.describe(now)).collect()
    }

    pub(crate) fn set(&self, text: &str, when: When, repeat: Repeat) -> Outcome {
        let now = (self.now)();
        let mut list = self.lock();
        let before = list.clone();
        let reminder = match list.add(now, text, when, repeat) {
            Ok(reminder) => reminder,
            Err(why) => return Outcome::failed(format!("Not set: {why}.")),
        };
        if let Err(err) = self.save(&list) {
            log::warn!("cannot save the reminders: {err}");
            *list = before;
            return Outcome::failed("The reminder could not be saved.");
        }
        drop(list);
        (self.changed)();
        let (id, when) = (reminder.id, reminder.when(now));
        Outcome::done(format!("Reminder {id} set for {when}."))
    }

    pub(crate) fn tell(&self) -> Outcome {
        let lines = self.lines();
        if lines.is_empty() {
            return Outcome::done("There are no reminders.");
        }
        Outcome::done(lines.join("\n"))
    }

    pub(crate) fn cancel(&self, id: u32) -> Outcome {
        let mut list = self.lock();
        let before = list.clone();
        let Some(reminder) = list.cancel(id) else {
            return Outcome::failed(format!("There is no reminder {id}."));
        };
        if let Err(err) = self.save(&list) {
            log::warn!("cannot save the reminders: {err}");
            *list = before;
            return Outcome::failed("The change could not be saved.");
        }
        drop(list);
        (self.changed)();
        Outcome::done(format!("Cancelled: {}.", reminder.text))
    }

    fn lock(&self) -> MutexGuard<'_, Reminders> {
        self.list.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Through a temporary file, so there is never half a list.
    fn save(&self, list: &Reminders) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(folder) = file.parent() {
            fs::create_dir_all(folder)?;
        }
        let new = file.with_extension("json.new");
        fs::write(&new, serde_json::to_vec_pretty(list)?)?;
        fs::rename(&new, file)
    }
}

fn load(file: &Path) -> Reminders {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Reminders::default(),
        Err(err) => {
            log::warn!("cannot read the reminders: {err}");
            set_aside(file);
            return Reminders::default();
        }
    };
    match serde_json::from_str::<Reminders>(&text) {
        Ok(list) => list.checked(),
        Err(err) => {
            log::warn!(
                "the reminders are damaged (line {}, column {})",
                err.line(),
                err.column()
            );
            set_aside(file);
            Reminders::default()
        }
    }
}

/// Keeps what cannot be read: the next save must not overwrite it.
fn set_aside(file: &Path) {
    if let Err(err) = fs::rename(file, file.with_extension("bad.json")) {
        log::warn!("cannot move the reminders aside: {err}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A file of this test's own, in an empty folder.
    fn file(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("sohi-agenda-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder.join("reminders.json")
    }

    fn now() -> Stamp {
        "2026-11-24 18:00".parse().unwrap()
    }

    fn open(file: &Path) -> (Agenda, Arc<AtomicUsize>) {
        let changes = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&changes);
        let agenda = Agenda::open(file.to_owned(), move || {
            count.fetch_add(1, Ordering::Relaxed);
        });
        (Agenda { now, ..agenda }, changes)
    }

    #[test]
    fn reminders_survive_a_restart() {
        let file = file("restart");
        let (agenda, changes) = open(&file);
        assert_eq!(agenda.tell().message, "There are no reminders.");
        let set = agenda.set("call \"Mum\"", When::In(600), Repeat::Daily);
        let want = "Reminder 1 set for today at 18:10, every day.";
        assert_eq!(set, Outcome::done(want));
        assert!(!agenda.set("x", When::In(0), Repeat::Once).ok);
        assert_eq!(changes.load(Ordering::Relaxed), 1);

        let (agenda, _) = open(&file);
        let want = ["1: call \"Mum\", today at 18:10, every day"];
        assert_eq!(agenda.lines(), want);
        assert_eq!(agenda.next_at(), Some(now().plus(600)));
        assert_eq!(agenda.take_due(now().plus(600)).len(), 1);

        let (agenda, changes) = open(&file);
        let want = ["1: call \"Mum\", tomorrow at 18:10, every day"];
        assert_eq!(agenda.lines(), want);
        assert!(!agenda.cancel(2).ok);
        assert!(agenda.cancel(1).ok);
        assert_eq!(changes.load(Ordering::Relaxed), 1);
        assert_eq!(open(&file).0.next_at(), None);
        fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_damaged_file_is_kept() {
        let file = file("damaged");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, r#"[{"id":1,"at":"soon","text":"x"}]"#).unwrap();
        let (agenda, _) = open(&file);
        assert_eq!(agenda.next_at(), None);
        assert!(agenda.set("y", When::In(60), Repeat::Once).ok);
        let kept = fs::read_to_string(file.with_extension("bad.json")).unwrap();
        assert!(kept.contains("soon"));
        fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn what_cannot_be_saved_is_not_set() {
        let file = file("unsaved");
        // A folder in the way.
        fs::create_dir_all(file.with_extension("json.new")).unwrap();
        let (agenda, changes) = open(&file);
        assert!(!agenda.set("x", When::In(60), Repeat::Once).ok);
        assert_eq!(agenda.next_at(), None);
        assert_eq!(changes.load(Ordering::Relaxed), 0);
        fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }
}
