//! `--perf` mode: machine-readable timing lines for `xtask perf` (spec §6).
//!
//! Each metric is printed to stdout as `perf: <key>=<value>` and mirrored to
//! the log. Once armed, the named event `Local\Sohi.Armed` is set. After an
//! idle sample of `idle` seconds, the app window is closed and Sohi exits.

use std::time::Duration;

use sohi_win::perf::{SignalEvent, memory_usage, process_times};
use sohi_win::{RemoteWindow, WinError};

/// Name of the event set once Sohi is armed.
const ARMED_EVENT: &str = r"Local\Sohi.Armed";

/// Default idle sample length (the §6 CPU budget is per 60 s).
pub const DEFAULT_IDLE: Duration = Duration::from_secs(60);

/// Wait before the idle sample so the launch peek (about 1.7 s) has
/// finished and the app is parked.
const SETTLE: Duration = Duration::from_millis(2500);

fn emit(key: &str, value: impl std::fmt::Display) {
    println!("perf: {key}={value}");
    log::info!("perf: {key}={value}");
}

#[allow(clippy::cast_precision_loss)] // ms values are far below 2^52.
fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Reports the first visible head frame (spec §6 cold start, second half).
pub fn on_first_frame() {
    match process_times() {
        Ok(times) => emit(
            "first_frame_ms",
            format_args!("{:.1}", millis(times.since_creation)),
        ),
        Err(err) => log::warn!("cannot time first frame: {err}"),
    }
}

/// Reports per-frame render + present cost for one reveal-to-park cycle.
pub fn on_head_frames(count: usize, p95: Duration, max: Duration) {
    emit("head_frames", count);
    emit("head_frame_p95_ms", format_args!("{:.3}", millis(p95)));
    emit("head_frame_max_ms", format_args!("{:.3}", millis(max)));
}

/// Reports the armed milestone and starts the idle sample.
///
/// Returns the armed event, which must stay alive until exit so observers
/// can still open it.
///
/// # Errors
/// Returns [`WinError`] if a metric or the event cannot be read or created.
pub fn on_armed(window: RemoteWindow, idle: Duration) -> Result<SignalEvent, WinError> {
    let times = process_times()?;
    let event = SignalEvent::create_signaled(ARMED_EVENT)?;
    emit(
        "armed_ms",
        format_args!("{:.1}", millis(times.since_creation)),
    );
    let mem = memory_usage()?;
    emit("armed_private_bytes", mem.private_bytes);
    emit("armed_working_set", mem.working_set);

    std::thread::Builder::new()
        .name("perf-idle".to_owned())
        .spawn(move || idle_sample(window, idle))
        .map_err(|err| log::error!("cannot start perf thread: {err}"))
        .ok();
    Ok(event)
}

fn idle_sample(window: RemoteWindow, idle: Duration) {
    let result = (|| -> Result<(), WinError> {
        std::thread::sleep(SETTLE);
        let start = process_times()?;
        // A blocking sleep on this helper thread; the app itself stays idle.
        std::thread::sleep(idle);
        let end = process_times()?;
        let mem = memory_usage()?;
        let wall = end.since_creation.saturating_sub(start.since_creation);
        emit("idle_wall_ms", format_args!("{:.0}", millis(wall)));
        emit(
            "idle_cpu_ms",
            format_args!("{:.1}", millis(end.cpu.saturating_sub(start.cpu))),
        );
        emit("idle_private_bytes", mem.private_bytes);
        emit("idle_working_set", mem.working_set);
        Ok(())
    })();
    if let Err(err) = result {
        log::error!("perf sample failed: {err}");
    }
    emit("done", 1);
    if let Err(err) = window.post_close() {
        log::error!("cannot close after perf sample: {err}");
    }
}
