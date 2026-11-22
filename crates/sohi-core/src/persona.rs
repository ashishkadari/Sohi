//! Sohi's persona and system prompts (spec §5.5).

use std::fmt::Write;

/// A local wall-clock time, read by the caller (`sohi-core` reads no
/// clocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalTime {
    /// e.g. 2026.
    pub year: u16,
    /// 1–12.
    pub month: u8,
    /// 1–31.
    pub day: u8,
    /// 0 = Sunday … 6 = Saturday.
    pub weekday: u8,
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
}

impl LocalTime {
    /// "Sunday 22 November 2026, 15:04".
    #[must_use]
    pub fn describe(&self) -> String {
        const DAYS: [&str; 7] = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        const MONTHS: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let day = DAYS.get(usize::from(self.weekday)).unwrap_or(&"");
        let month = MONTHS
            .get(usize::from(self.month.saturating_sub(1)))
            .unwrap_or(&"");
        format!(
            "{day} {} {month} {}, {:02}:{:02}",
            self.day, self.year, self.hour, self.minute
        )
    }
}

/// The system prompt for a typed conversation. `user_name` may be empty.
#[must_use]
pub fn text_system_prompt(user_name: &str, now: &LocalTime) -> String {
    let mut prompt = String::from(
        "You are Sohi, an assistant living on the user's Windows desktop. \
         You are a sharp-witted, calm American woman: a netrunner with dry, \
         sarcastic humour, a little secretive, and fiercely loyal to the user.",
    );
    let name = user_name.trim();
    if name.is_empty() {
        prompt.push_str(" Do not call the user by any name.");
    } else {
        let _ = write!(prompt, " Call the user {name}.");
    }
    let _ = write!(
        prompt,
        "\n\nThis is a typed chat. Be concise: a few sentences unless more is \
         asked for. Plain text reads best; use short lists or code blocks only \
         when they help.\n\nIt is now {} (the user's local time). You cannot \
         act on the computer yet; if asked to, say so briefly.",
        now.describe()
    );
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: LocalTime = LocalTime {
        year: 2026,
        month: 11,
        day: 22,
        weekday: 0,
        hour: 9,
        minute: 5,
    };

    #[test]
    fn describes_the_time() {
        assert_eq!(NOW.describe(), "Sunday 22 November 2026, 09:05");
    }

    #[test]
    fn the_prompt_uses_the_name_and_the_time() {
        let prompt = text_system_prompt("Ashish", &NOW);
        assert!(prompt.contains("Call the user Ashish."));
        assert!(prompt.contains("Sunday 22 November 2026, 09:05"));
        assert!(prompt.contains("netrunner"));
    }

    #[test]
    fn an_empty_name_is_never_invented() {
        let prompt = text_system_prompt("  ", &NOW);
        assert!(prompt.contains("Do not call the user by any name."));
    }

    #[test]
    fn out_of_range_fields_do_not_panic() {
        let odd = LocalTime {
            month: 0,
            weekday: 9,
            ..NOW
        };
        let _ = odd.describe();
    }
}
