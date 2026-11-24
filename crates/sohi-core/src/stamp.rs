//! Wall-clock moments and the calendar (spec §5.10). Like everything in
//! this crate it reads no clock: callers bring the time.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
/// Seconds in a day.
pub const DAY: i64 = 24 * HOUR;

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

/// What the clock on the wall shows at some moment, to the second. Time
/// zones and daylight saving time are not its business: 9 o'clock tomorrow
/// is 9 on the clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Stamp(i64);

/// A day of the calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date(i64);

/// A time of the day, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Clock(i64);

impl Date {
    /// A day, if it is in the calendar (years 1970 to 9999).
    #[must_use]
    pub fn new(year: i64, month: i64, day: i64) -> Option<Self> {
        if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || day < 1 {
            return None;
        }
        let date = Self(days_from_civil(year, month, day));
        // The 30th of February comes out as a day in March.
        (date.parts() == (year, month, day)).then_some(date)
    }

    /// Year, month and day.
    #[must_use]
    pub fn parts(self) -> (i64, i64, i64) {
        civil_from_days(self.0)
    }

    /// 0 = Sunday … 6 = Saturday.
    #[must_use]
    pub fn weekday(self) -> i64 {
        // 1970-01-01 was a Thursday.
        (self.0 + 4).rem_euclid(7)
    }

    /// Whether it is Saturday or Sunday.
    #[must_use]
    pub fn is_weekend(self) -> bool {
        matches!(self.weekday(), 0 | 6)
    }

    /// `days` later.
    #[must_use]
    pub fn plus(self, days: i64) -> Self {
        Self(self.0 + days)
    }

    /// The days from `earlier` to this.
    #[must_use]
    pub fn since(self, earlier: Self) -> i64 {
        self.0 - earlier.0
    }

    /// This day at `clock`.
    #[must_use]
    pub fn at(self, clock: Clock) -> Stamp {
        Stamp(self.0 * DAY + clock.0)
    }
}

impl Clock {
    /// A time of the day, if there is such a time.
    #[must_use]
    pub fn new(hour: i64, minute: i64, second: i64) -> Option<Self> {
        ((0..24).contains(&hour) && (0..60).contains(&minute) && (0..60).contains(&second))
            .then_some(Self(hour * HOUR + minute * MINUTE + second))
    }

    fn parts(self) -> (i64, i64, i64) {
        (self.0 / HOUR, self.0 % HOUR / MINUTE, self.0 % MINUTE)
    }
}

impl Stamp {
    /// Its day.
    #[must_use]
    pub fn date(self) -> Date {
        Date(self.0.div_euclid(DAY))
    }

    /// Its time of the day.
    #[must_use]
    pub fn clock(self) -> Clock {
        Clock(self.0.rem_euclid(DAY))
    }

    /// `seconds` later.
    #[must_use]
    pub fn plus(self, seconds: i64) -> Self {
        Self(self.0.saturating_add(seconds))
    }

    /// The seconds from `earlier` to this.
    #[must_use]
    pub fn since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }

    /// For a sentence said at `now`: "today at 17:30", "tomorrow at 09:00",
    /// "on Friday 27 November at 09:00".
    #[must_use]
    pub fn describe(self, now: Self) -> String {
        let (hour, minute, _) = self.clock().parts();
        let clock = format!("{hour:02}:{minute:02}");
        match self.date().since(now.date()) {
            0 => return format!("today at {clock}"),
            1 => return format!("tomorrow at {clock}"),
            -1 => return format!("yesterday at {clock}"),
            _ => {}
        }
        let (year, month, day) = self.date().parts();
        let weekday = weekday_name(self.date().weekday());
        let month = name(&MONTHS, month - 1);
        let year = if year == now.date().parts().0 {
            String::new()
        } else {
            format!(" {year}")
        };
        format!("on {weekday} {day} {month}{year} at {clock}")
    }
}

/// The weekday called `name` (0 = Sunday), in any case.
#[must_use]
pub fn weekday_named(name: &str) -> Option<i64> {
    let index = DAYS.iter().position(|d| d.eq_ignore_ascii_case(name))?;
    i64::try_from(index).ok()
}

/// What a weekday (0 = Sunday) is called.
#[must_use]
pub fn weekday_name(weekday: i64) -> &'static str {
    name(&DAYS, weekday)
}

fn name(names: &[&'static str], index: i64) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i).copied())
        .unwrap_or_default()
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = self.parts();
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

impl fmt::Display for Clock {
    /// "09:00".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (hour, minute, _) = self.parts();
        write!(f, "{hour:02}:{minute:02}")
    }
}

impl fmt::Display for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (hour, minute, second) = self.clock().parts();
        write!(f, "{} {hour:02}:{minute:02}:{second:02}", self.date())
    }
}

/// Not a date or a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a date or time")]
pub struct NotATime;

impl FromStr for Date {
    type Err = NotATime;

    /// "2026-11-24".
    fn from_str(text: &str) -> Result<Self, NotATime> {
        let mut parts = text.trim().splitn(3, '-').map(number);
        let mut next = || parts.next().flatten().ok_or(NotATime);
        Self::new(next()?, next()?, next()?).ok_or(NotATime)
    }
}

impl FromStr for Clock {
    type Err = NotATime;

    /// "09:00", "9", "9:30", "9 pm", "9:30pm", "17:30:15", "noon" or
    /// "midnight".
    fn from_str(text: &str) -> Result<Self, NotATime> {
        let text = text.trim().to_ascii_lowercase();
        match text.as_str() {
            "noon" => return Self::new(12, 0, 0).ok_or(NotATime),
            "midnight" => return Self::new(0, 0, 0).ok_or(NotATime),
            _ => {}
        }
        let text = text.replace("a.m.", "am").replace("p.m.", "pm");
        let (time, half) = match (text.strip_suffix("am"), text.strip_suffix("pm")) {
            (Some(time), _) => (time, Some(0)),
            (_, Some(time)) => (time, Some(12)),
            _ => (text.as_str(), None),
        };
        let mut parts = time.trim().split(':').map(number);
        let hour = parts.next().flatten().ok_or(NotATime)?;
        let minute = parts.next().map_or(Ok(0), |n| n.ok_or(NotATime))?;
        let second = parts.next().map_or(Ok(0), |n| n.ok_or(NotATime))?;
        let hour = match half {
            Some(half) if (1..=12).contains(&hour) => hour % 12 + half,
            Some(_) => return Err(NotATime),
            None => hour,
        };
        if parts.next().is_some() {
            return Err(NotATime);
        }
        Self::new(hour, minute, second).ok_or(NotATime)
    }
}

impl FromStr for Stamp {
    type Err = NotATime;

    /// "2026-11-24 17:30" (or with a `T` between, and seconds).
    fn from_str(text: &str) -> Result<Self, NotATime> {
        let (date, clock) = text.trim().split_once([' ', 'T']).ok_or(NotATime)?;
        Ok(date.parse::<Date>()?.at(clock.parse()?))
    }
}

impl TryFrom<String> for Stamp {
    type Error = NotATime;

    fn try_from(text: String) -> Result<Self, NotATime> {
        text.parse()
    }
}

impl From<Stamp> for String {
    fn from(stamp: Stamp) -> Self {
        stamp.to_string()
    }
}

/// A short run of digits.
fn number(text: &str) -> Option<i64> {
    let plain = (1..=4).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
    plain.then(|| text.parse().ok()).flatten()
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm, proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let of_era = y - era * 400;
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * of_era + of_era / 4 - of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Stamp {
        text.parse().unwrap()
    }

    #[test]
    fn the_calendar_is_right() {
        assert_eq!(Date::new(1970, 1, 1), Some(Date(0)));
        assert_eq!(Date::new(2000, 3, 1), Some(Date(11_017)));
        assert_eq!("2026-11-24".parse::<Date>().unwrap().weekday(), 2);
        for day in 0..60_000 {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
        for (y, m, d) in [(2026, 2, 29), (2100, 2, 29), (2026, 13, 1), (2026, 4, 31)] {
            assert_eq!(Date::new(y, m, d), None, "{y}-{m}-{d}");
        }
        assert!(Date::new(2028, 2, 29).is_some());
    }

    #[test]
    fn times_are_understood() {
        for (text, (h, m, s)) in [
            ("09:00", (9, 0, 0)),
            ("9", (9, 0, 0)),
            ("9 pm", (21, 0, 0)),
            ("9:30PM", (21, 30, 0)),
            ("12 am", (0, 0, 0)),
            ("12pm", (12, 0, 0)),
            ("8 a.m.", (8, 0, 0)),
            ("17:30:15", (17, 30, 15)),
            ("noon", (12, 0, 0)),
        ] {
            assert_eq!(text.parse(), Ok(Clock::new(h, m, s).unwrap()), "{text}");
        }
        for text in [
            "", "24:00", "9:60", "13 pm", "0 am", "nine", "-1", "9:", "9:0:0:0", "bed",
        ] {
            assert_eq!(text.parse::<Clock>(), Err(NotATime), "{text}");
        }
    }

    #[test]
    fn it_survives_being_written_down() {
        let stamp = at("2026-11-24T17:05:09");
        assert_eq!(stamp.to_string(), "2026-11-24 17:05:09");
        assert_eq!(at(&stamp.to_string()), stamp);
        assert_eq!(stamp.plus(DAY).since(stamp), DAY);
        for text in ["", "2026-11-24", "2026-02-30 10:00", "2026-11-24 25:00"] {
            assert_eq!(text.parse::<Stamp>(), Err(NotATime), "{text}");
        }
    }

    #[test]
    fn it_is_described_from_now() {
        let now = at("2026-11-24 18:00");
        for (when, want) in [
            ("2026-11-24 19:05", "today at 19:05"),
            ("2026-11-25 09:00", "tomorrow at 09:00"),
            ("2026-11-23 09:00", "yesterday at 09:00"),
            ("2026-11-27 09:00", "on Friday 27 November at 09:00"),
            ("2027-03-03 09:00", "on Wednesday 3 March 2027 at 09:00"),
        ] {
            assert_eq!(at(when).describe(now), want);
        }
    }
}
