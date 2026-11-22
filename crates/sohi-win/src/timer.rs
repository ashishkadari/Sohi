//! High-resolution frame pacing timer (spec §4.5).
//!
//! A `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` waitable timer, re-armed as a
//! one-shot after each tick against an absolute schedule, so periods like
//! 16.6 ms do not drift. It never calls `timeBeginPeriod`. When stopped the
//! timer is cancelled and the message loop blocks with no timeout at all.

use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW,
    SetWaitableTimer, TIMER_ALL_ACCESS,
};
use windows::core::PCWSTR;

use crate::{OwnedHandle, WinError};

/// Frame period at 60 fps.
pub const FRAME_60_FPS: Duration = Duration::from_micros(16_667);
/// Frame period at 30 fps.
pub const FRAME_30_FPS: Duration = Duration::from_micros(33_333);

/// Pacing state: `None` while stopped.
#[derive(Debug, Clone, Copy)]
struct Schedule {
    period: Duration,
    next: Instant,
}

/// The UI thread's frame timer. Methods take `&self` so the app and the
/// message loop can share it through an `Rc`.
#[derive(Debug)]
pub struct FrameTimer {
    handle: OwnedHandle,
    schedule: Cell<Option<Schedule>>,
}

impl FrameTimer {
    /// Creates a stopped timer.
    ///
    /// # Errors
    /// Returns [`WinError`] if the timer cannot be created.
    pub fn new() -> Result<Self, WinError> {
        // SAFETY: no name, default security; the flags request a
        // high-resolution auto-reset (synchronization) timer.
        let handle = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .map_err(|e| WinError::api("CreateWaitableTimerExW", &e))?;
        Ok(Self {
            // SAFETY: fresh handle that we now own.
            handle: unsafe { OwnedHandle::from_raw(handle) },
            schedule: Cell::new(None),
        })
    }

    /// Starts (or retimes) ticking every `period`. The first tick is one
    /// period from now.
    pub fn start(&self, period: Duration) {
        let next = Instant::now() + period;
        self.schedule.set(Some(Schedule { period, next }));
        self.arm(period);
    }

    /// Stops ticking and cancels any pending signal.
    pub fn stop(&self) {
        self.schedule.set(None);
        // SAFETY: valid timer handle.
        let _ = unsafe { CancelWaitableTimer(self.handle.raw()) };
    }

    /// Whether the timer is ticking.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.schedule.get().is_some()
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.handle.raw()
    }

    /// Called by the loop when the handle is signaled. Re-arms for the next
    /// slot and returns `true` if a frame should be produced.
    pub(crate) fn on_signaled(&self) -> bool {
        let Some(mut schedule) = self.schedule.get() else {
            return false; // stopped after the signal was queued
        };
        let now = Instant::now();
        schedule.next += schedule.period;
        if schedule.next <= now {
            // Fell behind (e.g. a long handler): skip missed slots.
            schedule.next = now + schedule.period;
        }
        self.schedule.set(Some(schedule));
        self.arm(schedule.next - now);
        true
    }

    fn arm(&self, due: Duration) {
        // Negative due time = relative, in 100 ns units.
        let ticks = i64::try_from(due.as_nanos() / 100)
            .unwrap_or(i64::MAX)
            .max(1);
        let due_time = -ticks;
        // SAFETY: valid timer handle; `due_time` outlives the call; one-shot
        // (period 0), no completion routine.
        let _ = unsafe {
            SetWaitableTimer(self.handle.raw(), &raw const due_time, 0, None, None, false)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::WaitForSingleObject;

    fn wait(timer: &FrameTimer, ms: u32) -> bool {
        // SAFETY: valid handle owned by `timer`.
        let r = unsafe { WaitForSingleObject(timer.raw(), ms) };
        assert!(r == WAIT_OBJECT_0 || r == WAIT_TIMEOUT, "{r:?}");
        r == WAIT_OBJECT_0
    }

    #[test]
    fn ticks_at_roughly_the_requested_rate() {
        let timer = FrameTimer::new().unwrap();
        let start = Instant::now();
        timer.start(FRAME_60_FPS);
        for _ in 0..12 {
            assert!(wait(&timer, 1000));
            assert!(timer.on_signaled());
        }
        let elapsed = start.elapsed();
        // 12 x 16.7 ms = 200 ms; allow scheduler slop.
        assert!(elapsed >= Duration::from_millis(180), "{elapsed:?}");
        assert!(elapsed <= Duration::from_millis(400), "{elapsed:?}");
    }

    #[test]
    fn stopped_timer_never_signals() {
        let timer = FrameTimer::new().unwrap();
        timer.start(Duration::from_millis(5));
        timer.stop();
        assert!(!timer.is_running());
        assert!(!wait(&timer, 50));
        assert!(!timer.on_signaled());
    }
}
