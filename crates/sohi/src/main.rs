//! Sohi binary: wires the crates together and runs the UI-thread loop.

#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

mod logger;

use std::process::ExitCode;

use sohi_win::{AppWindow, SingleInstance, WinEvent};

/// Named mutex that marks the running instance (spec §5.8).
const INSTANCE_MUTEX: &str = r"Local\Sohi.SingleInstance";
/// Window class of the hidden app window; a second launch finds it by name.
const APP_WINDOW_CLASS: &str = "Sohi.App";
/// Registered message a second launch posts to wake the running instance.
const WAKE_MESSAGE: &str = "Sohi.Wake";

/// Command-line options. Unknown arguments are logged and ignored.
#[derive(Debug, Default)]
struct Args {
    /// Panic right after startup to exercise the panic hook (P0 acceptance).
    test_panic: bool,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> (Self, Vec<String>) {
        let mut parsed = Self::default();
        let mut unknown = Vec::new();
        for arg in args {
            match arg.as_str() {
                "--test-panic" => parsed.test_panic = true,
                _ => unknown.push(arg),
            }
        }
        (parsed, unknown)
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            log::error!("fatal: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, sohi_win::WinError> {
    let wake_message = sohi_win::register_window_message(WAKE_MESSAGE)?;
    let Some(_instance) = SingleInstance::acquire(INSTANCE_MUTEX)? else {
        // Another instance owns the mutex: ask it to wake and leave at once.
        // The second process never opens the log, so rotation has one owner.
        sohi_win::forward_to_running_instance(APP_WINDOW_CLASS, wake_message)?;
        return Ok(ExitCode::SUCCESS);
    };

    // Logging is best effort: Sohi still runs if the log dir is unwritable.
    let _ = logger::init(&logger::default_dir(), log::LevelFilter::Info);
    logger::install_panic_hook();
    log::info!("sohi {} starting", env!("CARGO_PKG_VERSION"));

    let (args, unknown) = Args::parse(std::env::args().skip(1));
    if !unknown.is_empty() {
        log::warn!("ignoring unknown arguments: {unknown:?}");
    }
    assert!(!args.test_panic, "--test-panic requested");

    let _window = AppWindow::create(APP_WINDOW_CLASS, wake_message, |event| match event {
        WinEvent::WakeRequested => log::info!("wake requested"),
    })?;
    let code = sohi_win::run_message_loop();
    log::info!("exiting with code {code}");
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}
