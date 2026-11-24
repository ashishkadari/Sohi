//! The arguments of the reminder tools.

use serde_json::{Map, Value};
use sohi_core::reminder::{Day, Repeat, When};
use sohi_core::stamp::{DAY, weekday_name};

use super::CallError;

/// The longest span that is accepted, in minutes; the agenda has the real
/// limit.
const MAX_MINUTES: f64 = 10_000_000.0;

fn bad(why: &str) -> CallError {
    CallError::BadArguments(why.to_owned())
}

/// `key` as text; nothing, `null` and an empty text are all nothing.
fn optional<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, CallError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(text)) => Ok(text.trim()),
        Some(_) => Err(bad(&format!("\"{key}\" must be a string"))),
    }
}

/// `in_minutes`, if it says anything. Models that have to fill it in write
/// 0.
fn minutes(args: &Map<String, Value>) -> Result<Option<f64>, CallError> {
    let minutes = match args.get("in_minutes") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(text)) if text.trim().is_empty() => return Ok(None),
        // Small models sometimes quote numbers.
        Some(Value::String(text)) => text.trim().parse().ok(),
        Some(_) => None,
    };
    match minutes {
        Some(0.0) => Ok(None),
        Some(n) if n > 0.0 && n <= MAX_MINUTES => Ok(Some(n)),
        _ => Err(bad("\"in_minutes\" must be a number above 0")),
    }
}

/// The repetition some models write into `day`.
fn repeat_in_day(day: &str) -> Option<Repeat> {
    match day.to_ascii_lowercase().as_str() {
        "weekday" | "weekdays" | "every weekday" => Some(Repeat::Weekdays),
        "daily" | "every day" | "everyday" => Some(Repeat::Daily),
        _ => None,
    }
}

/// When a reminder is wanted.
pub(super) fn when(args: &Map<String, Value>) -> Result<When, CallError> {
    if let Some(minutes) = minutes(args)? {
        // In range: checked above.
        #[expect(clippy::cast_possible_truncation)]
        return Ok(When::In((minutes * 60.0).round() as i64));
    }
    let time = optional(args, "time")?;
    if time.is_empty() {
        return Err(bad("give \"in_minutes\" or \"time\""));
    }
    let clock = time
        .parse()
        .map_err(|_| bad("\"time\" must be HH:MM, 24-hour"))?;
    let day = optional(args, "day")?;
    if repeat_in_day(day).is_some() {
        return Ok(When::At(clock, Day::Next));
    }
    let day = day
        .parse()
        .map_err(|_| bad("\"day\" must be today, tomorrow, a weekday's name or YYYY-MM-DD; for every weekday use \"repeat\""))?;
    Ok(When::At(clock, day))
}

/// How often.
pub(super) fn repeat(args: &Map<String, Value>) -> Result<Repeat, CallError> {
    if let Some(repeat) = repeat_in_day(optional(args, "day")?) {
        return Ok(repeat);
    }
    Repeat::named(optional(args, "repeat")?)
        .ok_or_else(|| bad("\"repeat\" must be once, daily, weekdays or weekly"))
}

/// Which one.
pub(super) fn id(args: &Map<String, Value>) -> Result<u32, CallError> {
    let id = match args.get("number") {
        Some(Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    };
    id.ok_or_else(|| bad("\"number\" must be a reminder's number"))
}

/// "in 10 min", "at a time" for a question.
pub(super) fn span(when: When) -> String {
    match when {
        When::In(seconds) if seconds < DAY => format!("in {} min", (seconds + 59) / 60),
        When::In(seconds) => format!("in {} days", seconds / DAY),
        When::At(clock, Day::Next) => format!("at {clock}"),
        When::At(clock, Day::Ahead(0)) => format!("today at {clock}"),
        When::At(clock, Day::Ahead(_)) => format!("tomorrow at {clock}"),
        When::At(clock, Day::On(date)) => format!("on {date} at {clock}"),
        When::At(clock, Day::Weekday(day)) => format!("on {} at {clock}", weekday_name(day)),
    }
}
