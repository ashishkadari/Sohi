//! Reminders (spec §5.10): what there is to remember, and when.

mod phrase;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::stamp::{Clock, DAY, Date, Stamp};

pub use phrase::{Phrase, parse};

/// The longest text, in characters.
pub const MAX_TEXT: usize = 120;
/// How many there can be.
pub const MAX_COUNT: usize = 100;
/// How far ahead, in seconds: five years.
const MAX_AHEAD: i64 = 5 * 366 * DAY;
/// From this many seconds on a reminder is late.
const LATE: i64 = 120;

/// How often a reminder comes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    /// Once.
    #[default]
    Once,
    /// Every day.
    Daily,
    /// Monday to Friday.
    Weekdays,
    /// Every week.
    Weekly,
}

impl Repeat {
    /// The repetition called `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "" | "once" | "none" | "never" => Self::Once,
            "daily" => Self::Daily,
            "weekdays" => Self::Weekdays,
            "weekly" => Self::Weekly,
            _ => return None,
        })
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Once => "",
            Self::Daily => ", every day",
            Self::Weekdays => ", Monday to Friday",
            Self::Weekly => ", every week",
        }
    }

    /// The first time after `now` it comes again, having come at `at`.
    fn next(self, at: Stamp, now: Stamp) -> Option<Stamp> {
        let step = match self {
            Self::Once => return None,
            Self::Daily | Self::Weekdays => 1,
            Self::Weekly => 7,
        };
        // However long the PC was off.
        let gone = now.date().since(at.date()).max(0);
        let mut date = at.date().plus(gone / step * step);
        while date.at(at.clock()) <= now || (self == Self::Weekdays && date.is_weekend()) {
            date = date.plus(step);
        }
        Some(date.at(at.clock()))
    }
}

/// When a reminder is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// This many seconds from now.
    In(i64),
    /// At this time, on that day or the next time the clock shows it.
    At(Clock, Option<Date>),
}

/// A reminder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    /// What it is called in a list.
    pub id: u32,
    /// When it is due next.
    pub at: Stamp,
    /// How often it comes.
    #[serde(default)]
    pub repeat: Repeat,
    /// What to remember.
    pub text: String,
}

impl Reminder {
    /// "2: call mum, tomorrow at 09:00, every week".
    #[must_use]
    pub fn describe(&self, now: Stamp) -> String {
        format!("{}: {}, {}", self.id, self.text, self.when(now))
    }

    /// "tomorrow at 09:00, every week".
    #[must_use]
    pub fn when(&self, now: Stamp) -> String {
        self.at.describe(now) + self.repeat.describe()
    }
}

/// Why a reminder is refused. Written for whoever asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// There is nothing to remember.
    NoText,
    /// The text is too long.
    TooLong,
    /// The moment has gone by.
    Past,
    /// It is too far away.
    TooFar,
    /// There are too many.
    TooMany,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoText => f.write_str("there is no text"),
            Self::TooLong => write!(f, "the text may have at most {MAX_TEXT} characters"),
            Self::Past => f.write_str("that moment is in the past"),
            Self::TooFar => f.write_str("that is more than 5 years away"),
            Self::TooMany => write!(f, "there are {MAX_COUNT} reminders already"),
        }
    }
}

impl std::error::Error for Refused {}

/// All reminders, the next one first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Reminders(Vec<Reminder>);

impl Reminders {
    /// Them all.
    #[must_use]
    pub fn all(&self) -> &[Reminder] {
        &self.0
    }

    /// When the next one is due.
    #[must_use]
    pub fn next_at(&self) -> Option<Stamp> {
        self.0.iter().map(|r| r.at).min()
    }

    /// Adds one.
    ///
    /// # Errors
    /// Returns why it cannot be.
    pub fn add(
        &mut self,
        now: Stamp,
        text: &str,
        when: When,
        repeat: Repeat,
    ) -> Result<Reminder, Refused> {
        let text = line(text);
        if text.is_empty() {
            return Err(Refused::NoText);
        }
        if text.chars().count() > MAX_TEXT {
            return Err(Refused::TooLong);
        }
        if self.0.len() >= MAX_COUNT {
            return Err(Refused::TooMany);
        }
        let at = resolve(now, when, repeat)?;
        let reminder = Reminder {
            id: self.0.iter().map(|r| r.id).max().unwrap_or(0) + 1,
            at,
            repeat,
            text,
        };
        self.0.push(reminder.clone());
        self.sort();
        Ok(reminder)
    }

    /// Takes one away for good.
    pub fn cancel(&mut self, id: u32) -> Option<Reminder> {
        let index = self.0.iter().position(|r| r.id == id)?;
        Some(self.0.remove(index))
    }

    /// Takes what is due at `now`. What repeats moves on to its next time
    /// after `now`, so it is due once, however much was missed.
    pub fn take_due(&mut self, now: Stamp) -> Vec<Reminder> {
        let (due, mut rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|r| r.at <= now);
        rest.extend(due.iter().filter_map(|r| {
            let at = r.repeat.next(r.at, now)?;
            Some(Reminder { at, ..r.clone() })
        }));
        self.0 = rest;
        self.sort();
        due
    }

    /// Tidies what was read from a file, which anyone may have edited.
    #[must_use]
    pub fn checked(mut self) -> Self {
        self.0.truncate(MAX_COUNT);
        for (reminder, id) in self.0.iter_mut().zip(1..) {
            reminder.id = id;
            reminder.text = line(&reminder.text).chars().take(MAX_TEXT).collect();
        }
        self.0.retain(|r| !r.text.is_empty());
        self.sort();
        self
    }

    fn sort(&mut self) {
        self.0.sort_by_key(|r| (r.at, r.id));
    }
}

/// `text` on one line, without anything invisible.
fn line(text: &str) -> String {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect();
    words.join(" ")
}

fn resolve(now: Stamp, when: When, repeat: Repeat) -> Result<Stamp, Refused> {
    let at = match when {
        When::In(seconds) => now.plus(seconds),
        When::At(clock, Some(date)) => date.at(clock),
        When::At(clock, None) => {
            let mut date = now.date();
            let weekdays = repeat == Repeat::Weekdays;
            while date.at(clock) <= now || (weekdays && date.is_weekend()) {
                date = date.plus(1);
            }
            date.at(clock)
        }
    };
    match at.since(now) {
        ..=0 => Err(Refused::Past),
        ahead if ahead > MAX_AHEAD => Err(Refused::TooFar),
        _ => Ok(at),
    }
}

/// What to say at `now` about the reminders that came due.
#[must_use]
pub fn announce(due: &[Reminder], now: Stamp) -> String {
    let each: Vec<String> = due.iter().map(|r| sentence(r, now)).collect();
    let label = if each.len() == 1 {
        "Reminder"
    } else {
        "Reminders"
    };
    format!("{label}: {}", each.join(" "))
}

fn sentence(reminder: &Reminder, now: Stamp) -> String {
    let text = reminder.text.trim_end_matches(['.', '!', '?']);
    let end = reminder.text.chars().last();
    let end = end.filter(|c| matches!(c, '!' | '?')).unwrap_or('.');
    if now.since(reminder.at) < LATE {
        format!("{text}{end}")
    } else {
        format!("{text} ({}){end}", reminder.at.describe(now))
    }
}

#[cfg(test)]
mod tests;
