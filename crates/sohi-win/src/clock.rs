//! The local wall clock.

use sohi_core::persona::LocalTime;
use sohi_core::stamp::{Clock, Date, Stamp};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// The current local date and time.
#[must_use]
pub fn local_time() -> LocalTime {
    // SAFETY: no preconditions.
    let t = unsafe { GetLocalTime() };
    let byte = |v: u16| u8::try_from(v).unwrap_or(0);
    LocalTime {
        year: t.wYear,
        month: byte(t.wMonth),
        day: byte(t.wDay),
        weekday: byte(t.wDayOfWeek),
        hour: byte(t.wHour),
        minute: byte(t.wMinute),
    }
}

/// What the clock shows right now.
#[must_use]
pub fn local_stamp() -> Stamp {
    // SAFETY: no preconditions.
    let t = unsafe { GetLocalTime() };
    let date = Date::new(t.wYear.into(), t.wMonth.into(), t.wDay.into());
    let clock = Clock::new(t.wHour.into(), t.wMinute.into(), t.wSecond.into());
    // Windows only has real dates and times.
    match (date, clock) {
        (Some(date), Some(clock)) => date.at(clock),
        _ => Stamp::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_plausible_time() {
        let now = local_time();
        assert!(now.year >= 2024);
        assert!((1..=12).contains(&now.month));
        assert!(now.weekday <= 6 && now.hour <= 23 && now.minute <= 59);
    }
}
