//! Process metrics and the armed event for `--perf` mode (spec §6).

use std::time::Duration;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetProcessTimes, SetEvent,
};
use windows::core::HSTRING;

use crate::{OwnedHandle, WinError};

/// Memory counters for the current process, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    /// Private committed bytes (`PrivateUsage`).
    pub private_bytes: u64,
    /// Current working set.
    pub working_set: u64,
}

/// Time accounting for the current process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessTimes {
    /// Wall time since the process was created.
    pub since_creation: Duration,
    /// Kernel plus user CPU time consumed so far.
    pub cpu: Duration,
}

fn filetime_ticks(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

fn ticks_to_duration(ticks: u64) -> Duration {
    // FILETIME ticks are 100 ns.
    Duration::from_nanos(ticks.saturating_mul(100))
}

/// Reads CPU time and age of the current process.
///
/// # Errors
/// Returns [`WinError`] if `GetProcessTimes` fails.
pub fn process_times() -> Result<ProcessTimes, WinError> {
    let (mut creation, mut exit, mut kernel, mut user) = Default::default();
    // SAFETY: the pseudo-handle is always valid for the current process and
    // all four out-pointers are valid FILETIMEs.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(|e| WinError::api("GetProcessTimes", &e))?;
    // SAFETY: no preconditions.
    let now = unsafe { GetSystemTimePreciseAsFileTime() };
    let age = filetime_ticks(now).saturating_sub(filetime_ticks(creation));
    let cpu = filetime_ticks(kernel).saturating_add(filetime_ticks(user));
    Ok(ProcessTimes {
        since_creation: ticks_to_duration(age),
        cpu: ticks_to_duration(cpu),
    })
}

/// Reads private bytes and working set of the current process.
///
/// # Errors
/// Returns [`WinError`] if `GetProcessMemoryInfo` fails.
pub fn memory_usage() -> Result<MemoryUsage, WinError> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(u32::MAX);
    counters.cb = size;
    // SAFETY: PROCESS_MEMORY_COUNTERS_EX starts with PROCESS_MEMORY_COUNTERS
    // and `cb` tells the API the larger struct is available.
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            size,
        )
    }
    .map_err(|e| WinError::api("GetProcessMemoryInfo", &e))?;
    Ok(MemoryUsage {
        private_bytes: counters.PrivateUsage as u64,
        working_set: counters.WorkingSetSize as u64,
    })
}

/// A named manual-reset event, set once to signal a milestone to an
/// external observer (e.g. `Sohi.Armed` for `xtask perf`).
#[derive(Debug)]
pub struct SignalEvent {
    _handle: OwnedHandle,
}

impl SignalEvent {
    /// Creates the named event and sets it.
    ///
    /// # Errors
    /// Returns [`WinError`] if the event cannot be created or set.
    pub fn create_signaled(name: &str) -> Result<Self, WinError> {
        let name = HSTRING::from(name);
        // SAFETY: `name` is a valid wide string; manual reset, initially
        // unsignaled, default security.
        let handle = unsafe { CreateEventW(None, true, false, &name) }
            .map_err(|e| WinError::api("CreateEventW", &e))?;
        // SAFETY: CreateEventW returned a fresh handle that we now own.
        let owned = unsafe { OwnedHandle::from_raw(handle) };
        // SAFETY: `owned` is a valid event handle.
        unsafe { SetEvent(owned.raw()) }.map_err(|e| WinError::api("SetEvent", &e))?;
        Ok(Self { _handle: owned })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_times_are_plausible() {
        let t = process_times().unwrap();
        assert!(t.since_creation > Duration::ZERO);
        assert!(t.since_creation < Duration::from_secs(3600));
    }

    #[test]
    fn memory_usage_is_nonzero() {
        let m = memory_usage().unwrap();
        assert!(m.private_bytes > 0);
        assert!(m.working_set > 0);
    }
}
