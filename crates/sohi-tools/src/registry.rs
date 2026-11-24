//! The tools there are (spec §5.6), as the models are told about them.

use sohi_core::chat::ToolSpec;

/// A tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Opens an installed app.
    LaunchApp,
    /// Opens a web address in the default browser.
    OpenUrl,
    /// Reads the clipboard's text.
    ClipboardGet,
    /// Puts text on the clipboard.
    ClipboardSet,
    /// Presses a media key.
    Media,
    /// Changes the volume.
    Volume,
    /// Sets a reminder.
    ReminderSet,
    /// Lists the reminders.
    ReminderList,
    /// Cancels a reminder.
    ReminderCancel,
    /// Runs a command.
    #[cfg(feature = "shell")]
    ShellExec,
}

impl Tool {
    /// Every tool in this build.
    pub const ALL: &[Self] = &[
        Self::LaunchApp,
        Self::OpenUrl,
        Self::ClipboardGet,
        Self::ClipboardSet,
        Self::Media,
        Self::Volume,
        Self::ReminderSet,
        Self::ReminderList,
        Self::ReminderCancel,
        #[cfg(feature = "shell")]
        Self::ShellExec,
    ];

    /// The name models call it by, also used in `[tools]`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::LaunchApp => "launch_app",
            Self::OpenUrl => "open_url",
            Self::ClipboardGet => "clipboard_get",
            Self::ClipboardSet => "clipboard_set",
            Self::Media => "media",
            Self::Volume => "volume",
            Self::ReminderSet => "reminder_set",
            Self::ReminderList => "reminder_list",
            Self::ReminderCancel => "reminder_cancel",
            #[cfg(feature = "shell")]
            Self::ShellExec => "shell_exec",
        }
    }

    /// The tool called `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|tool| tool.name() == name)
    }

    /// What a model is told about it.
    #[must_use]
    pub fn spec(self) -> ToolSpec {
        let (description, parameters) = match self {
            Self::LaunchApp => (
                "Open an app installed on the user's PC, by its name.",
                r#"{"type":"object","properties":{"name":{"type":"string","description":"The app's name, e.g. Notepad"}},"required":["name"]}"#,
            ),
            Self::OpenUrl => (
                "Open a web page in the user's browser.",
                r#"{"type":"object","properties":{"url":{"type":"string","description":"An http or https address"}},"required":["url"]}"#,
            ),
            Self::ClipboardGet => (
                "Read the text on the user's clipboard. It may have changed since you last read or set it, so call this every time you are asked about it. The user is asked first.",
                r#"{"type":"object","properties":{}}"#,
            ),
            Self::ClipboardSet => (
                "Put text on the user's clipboard, replacing what is there.",
                r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#,
            ),
            Self::Media => (
                "Play or pause music and videos, skip a track, or mute the PC's sound. The user may have changed what is playing, so call this every time you are asked, also when you think it is done already. mute switches the sound off or back on.",
                r#"{"type":"object","properties":{"action":{"type":"string","enum":["play","pause","next","prev","mute"]}},"required":["action"]}"#,
            ),
            Self::Volume => (
                "Change the PC's volume: one step up or down, or set it to a level. One step is 10 percent.",
                r#"{"type":"object","properties":{"action":{"type":"string","enum":["up","down","set"]},"level":{"type":"integer","minimum":0,"maximum":100,"description":"Percent; only for set"}},"required":["action"]}"#,
            ),
            Self::ReminderSet => (
                "Set a reminder or a timer; the user is told when it is due. For a span of time give in_minutes (also fractions); otherwise give time, and day unless it is simply the next time the clock shows that time.",
                r#"{"type":"object","properties":{"text":{"type":"string","description":"What to remember, at most 120 characters, as it will be said to the user, e.g. call the dentist"},"in_minutes":{"type":"number"},"time":{"type":"string","description":"24-hour HH:MM"},"day":{"type":"string","description":"today, tomorrow, a weekday such as friday, or YYYY-MM-DD"},"repeat":{"type":"string","enum":["once","daily","weekdays","weekly"]}},"required":["text"]}"#,
            ),
            Self::ReminderList => (
                "List the user's reminders with their numbers and times. Call it every time you are asked: reminders go when they are due.",
                r#"{"type":"object","properties":{}}"#,
            ),
            Self::ReminderCancel => (
                "Cancel a reminder by its number. List the reminders first if you do not know it.",
                r#"{"type":"object","properties":{"number":{"type":"integer","minimum":1}},"required":["number"]}"#,
            ),
            #[cfg(feature = "shell")]
            Self::ShellExec => (
                "Run a command with cmd.exe and return its output. One line, at most 300 characters. The user is always asked first.",
                r#"{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}"#,
            ),
        };
        ToolSpec {
            name: self.name(),
            description,
            parameters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_unique() {
        for (i, tool) in Tool::ALL.iter().enumerate() {
            assert_eq!(Tool::named(tool.name()), Some(*tool));
            assert!(Tool::ALL[..i].iter().all(|t| t.name() != tool.name()));
        }
        assert_eq!(Tool::named("format_disk"), None);
        assert_eq!(Tool::named(""), None);
        assert_eq!(Tool::named("shell_exec").is_some(), cfg!(feature = "shell"));
    }

    #[test]
    fn every_schema_is_a_json_object_schema() {
        for tool in Tool::ALL {
            let spec = tool.spec();
            let schema: serde_json::Value = serde_json::from_str(spec.parameters).unwrap();
            assert_eq!(schema["type"], "object", "{}", spec.name);
            let properties = schema["properties"].as_object().unwrap();
            for required in schema["required"].as_array().into_iter().flatten() {
                assert!(properties.contains_key(required.as_str().unwrap()));
            }
            assert!(spec.description.ends_with('.'));
        }
    }
}
