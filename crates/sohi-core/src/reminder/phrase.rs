//! Reminders without a model (spec §5.3, §5.10): whole sentences only.

use super::{Day, When};

/// Something to do about reminders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phrase {
    /// Set one.
    Set(String, When),
    /// List them.
    List,
    /// Cancel this one.
    Cancel(u32),
}

/// Understands `text`, without its closing punctuation.
#[must_use]
pub fn parse(text: &str) -> Option<Phrase> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let lower: Vec<&str> = lower.iter().map(String::as_str).collect();
    match lower[..] {
        ["reminders"] | ["my" | "list" | "show", "reminders"] | ["show", "my", "reminders"] => {
            Some(Phrase::List)
        }
        ["cancel" | "delete", "reminder", id] => id.parse().ok().map(Phrase::Cancel),
        ["timer", "for", n, unit] | ["set", "a", "timer", "for", n, unit] => {
            Some(Phrase::Set("Timer".to_owned(), span(n, unit)?))
        }
        ["remind", "me", "to", ..] => {
            // "… to <text> in 5 minutes", "… to <text> at 9 pm".
            let (text, when) = (3..words.len())
                .rev()
                .find_map(|i| Some((words.get(3..i)?, moment(lower.get(i..)?)?)))?;
            set(text, when)
        }
        ["remind", "me", ..] => {
            let to = lower.iter().position(|w| *w == "to")?;
            set(words.get(to + 1..)?, moment(lower.get(2..to)?)?)
        }
        _ => None,
    }
}

fn set(text: &[&str], when: When) -> Option<Phrase> {
    (!text.is_empty()).then(|| Phrase::Set(text.join(" "), when))
}

/// "in 5 minutes", "at 9 pm".
fn moment(words: &[&str]) -> Option<When> {
    match words {
        ["in", n, unit] => span(n, unit),
        ["at", time @ ..] if !time.is_empty() => {
            let clock = time.join(" ").parse().ok()?;
            Some(When::At(clock, Day::Next))
        }
        _ => None,
    }
}

fn span(n: &str, unit: &str) -> Option<When> {
    let n: i64 = match n {
        "a" | "an" => 1,
        n => n.parse().ok().filter(|n| (1..=10_000).contains(n))?,
    };
    let unit = match unit {
        "second" | "seconds" => 1,
        "minute" | "minutes" => 60,
        "hour" | "hours" => 3600,
        "day" | "days" => 24 * 3600,
        _ => return None,
    };
    Some(When::In(n * unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_sentences_are_understood() {
        let at = |text: &str| When::At(text.parse().unwrap(), Day::Next);
        let set = |text: &str, when| Phrase::Set(text.to_owned(), when);
        for (text, want) in [
            (
                "remind me in 10 minutes to call Mum",
                set("call Mum", When::In(600)),
            ),
            (
                "Remind me to call Mum in an hour",
                set("call Mum", When::In(3600)),
            ),
            (
                "remind me at 9:30 pm to go to bed",
                set("go to bed", at("21:30")),
            ),
            (
                "remind me to go to bed at 9 pm",
                set("go to bed", at("21:00")),
            ),
            (
                "remind me to meet Bob at work at 17:00",
                set("meet Bob at work", at("17")),
            ),
            ("set a timer for 5 minutes", set("Timer", When::In(300))),
            ("my reminders", Phrase::List),
            ("Cancel reminder 3", Phrase::Cancel(3)),
        ] {
            assert_eq!(parse(text), Some(want), "{text}");
        }
    }

    #[test]
    fn anything_else_is_not() {
        for text in [
            "",
            "remind me",
            "remind me to call Mum",
            "remind me in 10 minutes",
            "remind me to in 10 minutes",
            "remind me to meet Bob at the office",
            "remind me in -5 minutes to x",
            "remind me in 10 lightyears to x",
            "what are my reminders for",
            "cancel reminder one",
            "set a timer",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
