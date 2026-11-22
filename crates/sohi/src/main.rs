//! Sohi binary: wires the crates together and runs the UI-thread loop.

#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

use sohi_win::{AppWindow, SingleInstance, WinEvent};

/// Named mutex that marks the running instance (spec §5.8).
const INSTANCE_MUTEX: &str = r"Local\Sohi.SingleInstance";
/// Window class of the hidden app window; a second launch finds it by name.
const APP_WINDOW_CLASS: &str = "Sohi.App";
/// Registered message a second launch posts to wake the running instance.
const WAKE_MESSAGE: &str = "Sohi.Wake";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            log::error!("fatal: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<std::process::ExitCode, sohi_win::WinError> {
    let wake_message = sohi_win::register_window_message(WAKE_MESSAGE)?;
    let Some(_instance) = SingleInstance::acquire(INSTANCE_MUTEX)? else {
        // Another instance owns the mutex: ask it to wake and leave at once.
        sohi_win::forward_to_running_instance(APP_WINDOW_CLASS, wake_message)?;
        return Ok(std::process::ExitCode::SUCCESS);
    };

    let _window = AppWindow::create(APP_WINDOW_CLASS, wake_message, |event| match event {
        WinEvent::WakeRequested => log::info!("wake requested"),
    })?;
    let code = sohi_win::run_message_loop();
    Ok(std::process::ExitCode::from(
        u8::try_from(code).unwrap_or(1),
    ))
}
