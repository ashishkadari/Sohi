//! Tool calls: a model's name and JSON arguments, checked and turned into
//! something that can be run. Anything that does not fit is an error the
//! model is told about, never a guess.

use std::fmt;

use serde_json::{Map, Value};

use crate::Tool;

/// The longest text argument that is accepted, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// How much of an argument a summary shows, in characters.
const SUMMARY_CHARS: usize = 120;

/// A checked tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `launch_app`.
    LaunchApp {
        /// What the app is called.
        name: String,
    },
    /// `open_url`.
    OpenUrl {
        /// The address, not validated yet.
        url: String,
    },
    /// `clipboard_get`.
    ClipboardGet,
    /// `clipboard_set`.
    ClipboardSet {
        /// What to copy.
        text: String,
    },
    /// `media`.
    Media(MediaAction),
    /// `volume`.
    Volume(VolumeAction),
    /// `shell_exec`.
    #[cfg(feature = "shell")]
    ShellExec {
        /// The command line.
        command: String,
    },
}

/// What happens to whatever is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    /// Play, or go on playing.
    Play,
    /// Pause.
    Pause,
    /// The next track.
    Next,
    /// The previous track.
    Previous,
    /// Sound off, or back on.
    Mute,
}

/// A volume change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeAction {
    /// A step louder.
    Up,
    /// A step quieter.
    Down,
    /// To this percentage.
    Set(u8),
}

/// Why a call cannot be run. Written for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// There is no such tool.
    UnknownTool,
    /// The arguments do not fit the tool's schema.
    BadArguments(String),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool => f.write_str("there is no such tool"),
            Self::BadArguments(why) => write!(f, "bad arguments: {why}"),
        }
    }
}

impl std::error::Error for CallError {}

impl Call {
    /// Checks a call. `arguments` is the JSON object a model sent; empty
    /// means no arguments.
    ///
    /// # Errors
    /// [`CallError`] if the tool does not exist or the arguments do not
    /// fit it.
    pub fn parse(tool: &str, arguments: &str) -> Result<Self, CallError> {
        let tool = Tool::named(tool).ok_or(CallError::UnknownTool)?;
        let args = object(arguments)?;
        Ok(match tool {
            Tool::LaunchApp => Self::LaunchApp {
                name: text(&args, "name")?,
            },
            Tool::OpenUrl => Self::OpenUrl {
                url: text(&args, "url")?,
            },
            Tool::ClipboardGet => Self::ClipboardGet,
            Tool::ClipboardSet => Self::ClipboardSet {
                text: text(&args, "text")?,
            },
            Tool::Media => Self::Media(match text(&args, "action")?.as_str() {
                "play" => MediaAction::Play,
                "pause" => MediaAction::Pause,
                "next" => MediaAction::Next,
                "prev" => MediaAction::Previous,
                "mute" => MediaAction::Mute,
                other => return Err(unknown_action(other)),
            }),
            Tool::Volume => Self::Volume(match text(&args, "action")?.as_str() {
                "up" => VolumeAction::Up,
                "down" => VolumeAction::Down,
                "set" => VolumeAction::Set(percent(&args)?),
                other => return Err(unknown_action(other)),
            }),
            #[cfg(feature = "shell")]
            Tool::ShellExec => {
                let command = text(&args, "command")?.trim().to_owned();
                crate::shell::check(&command)?;
                Self::ShellExec { command }
            }
        })
    }

    /// The tool it calls.
    #[must_use]
    pub fn tool(&self) -> Tool {
        match self {
            Self::LaunchApp { .. } => Tool::LaunchApp,
            Self::OpenUrl { .. } => Tool::OpenUrl,
            Self::ClipboardGet => Tool::ClipboardGet,
            Self::ClipboardSet { .. } => Tool::ClipboardSet,
            Self::Media(_) => Tool::Media,
            Self::Volume(_) => Tool::Volume,
            #[cfg(feature = "shell")]
            Self::ShellExec { .. } => Tool::ShellExec,
        }
    }

    /// One line for the user, to show or to ask about: "open Notepad".
    /// Arguments are shown on that one line, shortened, whatever is in
    /// them.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::LaunchApp { name } => format!("open {}", one_line(name)),
            Self::OpenUrl { url } => format!("open {}", one_line(url)),
            Self::ClipboardGet => "read your clipboard".to_owned(),
            Self::ClipboardSet { text } => format!("copy \"{}\"", one_line(text)),
            Self::Media(MediaAction::Play) => "play".to_owned(),
            Self::Media(MediaAction::Pause) => "pause".to_owned(),
            Self::Media(MediaAction::Next) => "next track".to_owned(),
            Self::Media(MediaAction::Previous) => "previous track".to_owned(),
            Self::Media(MediaAction::Mute) => "mute or unmute".to_owned(),
            Self::Volume(VolumeAction::Up) => "volume up".to_owned(),
            Self::Volume(VolumeAction::Down) => "volume down".to_owned(),
            Self::Volume(VolumeAction::Set(level)) => format!("volume {level}%"),
            // Checked to be one short visible line: shown whole.
            #[cfg(feature = "shell")]
            Self::ShellExec { command } => format!("command {command}"),
        }
    }
}

fn object(arguments: &str) -> Result<Map<String, Value>, CallError> {
    if arguments.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(arguments) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(CallError::BadArguments("not an object".to_owned())),
        Err(_) => Err(CallError::BadArguments("not valid JSON".to_owned())),
    }
}

fn text(args: &Map<String, Value>, key: &str) -> Result<String, CallError> {
    let value = args
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CallError::BadArguments(format!("\"{key}\" must be a string")))?;
    if value.trim().is_empty() {
        return Err(CallError::BadArguments(format!("\"{key}\" is empty")));
    }
    if value.len() > MAX_TEXT {
        return Err(CallError::BadArguments(format!("\"{key}\" is too long")));
    }
    Ok(value.to_owned())
}

fn percent(args: &Map<String, Value>) -> Result<u8, CallError> {
    let level = match args.get("level") {
        Some(Value::Number(n)) => n.as_u64(),
        // Small models sometimes quote numbers.
        Some(Value::String(s)) => s.trim().trim_end_matches('%').parse().ok(),
        _ => None,
    };
    level
        .and_then(|l| u8::try_from(l).ok())
        .filter(|l| *l <= 100)
        .ok_or_else(|| {
            CallError::BadArguments("\"level\" must be a whole number from 0 to 100".to_owned())
        })
}

fn unknown_action(action: &str) -> CallError {
    CallError::BadArguments(format!("there is no action \"{}\"", one_line(action)))
}

/// `text` as it can be shown on one line: control and invisible
/// formatting characters become spaces, and a long text is cut.
#[must_use]
pub fn one_line(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => visible(text.get(..cut).unwrap_or_default()) + "…",
        None => visible(text),
    }
}

/// `text` with spaces for control and invisible characters.
pub(crate) fn visible(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || is_invisible(c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Characters that could make a summary read differently from what runs:
/// line separators, zero-width characters and direction overrides.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad(tool: &str, args: &str) {
        let result = Call::parse(tool, args);
        assert!(
            matches!(result, Err(CallError::BadArguments(_))),
            "{tool} {args}: {result:?}"
        );
    }

    fn good(tool: &str, args: &str, want: &Call) {
        let call = Call::parse(tool, args).unwrap();
        assert_eq!(call.tool().name(), tool);
        assert_eq!(&call, want);
    }

    #[test]
    fn good_calls() {
        let name = "Notepad".to_owned();
        good(
            "launch_app",
            r#"{"name":"Notepad"}"#,
            &Call::LaunchApp { name },
        );
        let url = "https://github.com".to_owned();
        let args = r#"{"url":"https://github.com","extra":1}"#;
        good("open_url", args, &Call::OpenUrl { url });
        good("clipboard_get", "", &Call::ClipboardGet);
        good("clipboard_get", "{}", &Call::ClipboardGet);
        let text = " a\nb ".to_owned();
        good(
            "clipboard_set",
            r#"{"text":" a\nb "}"#,
            &Call::ClipboardSet { text },
        );
        let prev = Call::Media(MediaAction::Previous);
        good("media", r#"{"action":"prev"}"#, &prev);
        let pause = Call::Media(MediaAction::Pause);
        good("media", r#"{"action":"pause"}"#, &pause);
        let set = Call::Volume(VolumeAction::Set(30));
        good("volume", r#"{"action":"set","level":30}"#, &set);
        let full = Call::Volume(VolumeAction::Set(100));
        good("volume", r#"{"action":"set","level":"100%"}"#, &full);
        good(
            "volume",
            r#"{"action":"up"}"#,
            &Call::Volume(VolumeAction::Up),
        );
    }

    #[test]
    fn bad_calls() {
        assert_eq!(Call::parse("shutdown", "{}"), Err(CallError::UnknownTool));
        bad("launch_app", "{}");
        bad("launch_app", "[]");
        bad("launch_app", "{\"name\":");
        bad("launch_app", r#"{"name":7}"#);
        bad("launch_app", r#"{"name":"  "}"#);
        bad("media", r#"{"action":"explode"}"#);
        bad("media", r#"{"action":"play_pause"}"#);
        bad("volume", r#"{"action":"set"}"#);
        bad("volume", r#"{"action":"set","level":101}"#);
        bad("volume", r#"{"action":"set","level":-1}"#);
        bad("volume", r#"{"action":"set","level":2.5}"#);
        let long = format!(r#"{{"text":"{}"}}"#, "x".repeat(MAX_TEXT + 1));
        bad("clipboard_set", &long);
    }

    #[test]
    fn summaries_stay_on_one_line() {
        let call = Call::ClipboardSet {
            text: "a\nEnter = Yes\u{202E}txt".to_owned(),
        };
        assert_eq!(call.summary(), "copy \"a Enter = Yes txt\"");
        let call = Call::LaunchApp {
            name: "x".repeat(500),
        };
        assert!(call.summary().chars().count() < 140);
        assert_eq!(Call::Volume(VolumeAction::Set(30)).summary(), "volume 30%");
    }
}
