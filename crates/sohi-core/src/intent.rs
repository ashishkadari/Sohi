//! The offline command grammar (spec §5.3): what Sohi understands without
//! any model. Deterministic, and only whole sentences match, so ordinary
//! text is never taken for a command by accident.

use crate::reminder::{self, Phrase};

/// The reply to anything that is not a command.
pub const UNKNOWN_REPLY: &str = "Offline. I can only run local commands right now.";

/// A local command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// `open <app>`.
    Open(String),
    /// `go to <url>`.
    GoTo(String),
    /// `play`, `pause`, `next` or `previous`.
    Media(Media),
    /// `volume up`, `volume down` or `volume <N>`.
    Volume(Volume),
    /// `mute`.
    Mute,
    /// `copy <text>`.
    Copy(String),
    /// `what time is it`.
    Time,
    /// `remind me …`, `set a timer …`, `reminders`, `cancel reminder <N>`.
    Reminder(Phrase),
}

/// A media key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    /// Play.
    Play,
    /// Pause.
    Pause,
    /// Next track.
    Next,
    /// Previous track.
    Previous,
}

/// A volume change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Volume {
    /// Louder.
    Up,
    /// Quieter.
    Down,
    /// To this percentage (0–100).
    Set(u8),
}

/// Understands `text`, ignoring case, spacing, and closing punctuation.
#[must_use]
pub fn parse(text: &str) -> Option<Intent> {
    let text = text.trim();
    // What to copy is taken literally, punctuation included.
    if let Some(rest) = after_word(text, "copy") {
        return Some(Intent::Copy(rest.to_owned()));
    }
    let text = text.trim_end_matches(['.', '!', '?', ',', ' ']);
    if let Some(app) = after_word(text, "open") {
        return Some(Intent::Open(app.to_owned()));
    }
    if let Some(url) = after_word(text, "go").and_then(|rest| after_word(rest, "to")) {
        return is_address(url).then(|| Intent::GoTo(url.to_owned()));
    }
    if let Some(level) = after_word(text, "volume") {
        return volume(level).map(Intent::Volume);
    }
    if let Some(phrase) = reminder::parse(text) {
        return Some(Intent::Reminder(phrase));
    }
    let plain = text.to_lowercase();
    let plain: Vec<&str> = plain.split_whitespace().collect();
    Some(match plain[..] {
        ["play"] => Intent::Media(Media::Play),
        ["pause"] => Intent::Media(Media::Pause),
        ["next"] => Intent::Media(Media::Next),
        ["previous"] => Intent::Media(Media::Previous),
        ["mute"] => Intent::Mute,
        ["what", "time", "is", "it"] => Intent::Time,
        _ => return None,
    })
}

/// What follows `word` at the start of `text`, if anything does.
fn after_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let (first, rest) = text.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    (first.eq_ignore_ascii_case(word) && !rest.is_empty()).then_some(rest)
}

/// Whether `text` is one web address (a host with a dot, or http(s)).
fn is_address(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let host = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let host = host.split(['/', '?', '#']).next().unwrap_or_default();
    let labels: Vec<&str> = host.split('.').collect();
    !text.contains(char::is_whitespace)
        && !host.contains(['@', '\\', ':'])
        && labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty())
}

fn volume(level: &str) -> Option<Volume> {
    let level = level.to_lowercase();
    let number = level
        .trim_end_matches('%')
        .trim_end_matches("percent")
        .trim_end();
    match level.as_str() {
        "up" => Some(Volume::Up),
        "down" => Some(Volume::Down),
        _ => match number.parse() {
            Ok(n) if n <= 100 => Some(Volume::Set(n)),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn understands_every_command() {
        for (text, want) in [
            ("open Notepad", Intent::Open("Notepad".into())),
            (
                "Open visual studio code.",
                Intent::Open("visual studio code".into()),
            ),
            ("go to github.com", Intent::GoTo("github.com".into())),
            (
                "GO  TO https://x.y/z?q=1",
                Intent::GoTo("https://x.y/z?q=1".into()),
            ),
            ("play", Intent::Media(Media::Play)),
            ("Pause!", Intent::Media(Media::Pause)),
            ("next", Intent::Media(Media::Next)),
            ("previous", Intent::Media(Media::Previous)),
            ("volume up", Intent::Volume(Volume::Up)),
            ("Volume Down", Intent::Volume(Volume::Down)),
            ("volume 30", Intent::Volume(Volume::Set(30))),
            ("volume 30%", Intent::Volume(Volume::Set(30))),
            ("volume 5 percent", Intent::Volume(Volume::Set(5))),
            ("volume 100", Intent::Volume(Volume::Set(100))),
            ("mute", Intent::Mute),
            ("copy Hi there!", Intent::Copy("Hi there!".into())),
            ("What  time is it?", Intent::Time),
            ("Reminders.", Intent::Reminder(Phrase::List)),
        ] {
            assert_eq!(parse(text), Some(want), "{text:?}");
        }
    }

    #[test]
    fn anything_else_is_not_a_command() {
        for text in [
            "",
            "  ",
            "open",
            "open ?",
            "copy",
            "copy  ",
            "reopen notepad",
            "opening notepad",
            "go",
            "go to",
            "go to bed",
            "go to the shop",
            "go to .com",
            "go to javascript:alert(1)",
            "go to file:///c:/x.txt",
            "go to user@evil.com",
            "go away",
            "play some music",
            "volume",
            "volume 101",
            "volume -1",
            "volume loud",
            "unmute",
            "what time is it in Tokyo",
            "hello",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }
}
