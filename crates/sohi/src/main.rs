//! Sohi binary: wires the crates together and runs the UI-thread loop.

#![forbid(unsafe_code)]
#![windows_subsystem = "windows"]

mod actions;
mod app;
mod chat;
mod config_file;
mod corner;
mod dev;
mod full_mode;
mod head;
mod hotkeys;
mod logger;
mod offline;
mod offline_flag;
mod perf;
mod probe;
mod reply;
mod runtime;
mod set_key;
mod sounds;
mod speak;
mod stt;
mod text_mode;
mod ticker;
mod tray_menu;
mod voice;
mod yes_no;

use std::process::ExitCode;
use std::time::Duration;

use std::rc::Rc;

use sohi_tools::helper;
use sohi_win::{AppWindow, EventQueue, FrameTimer, SingleInstance};

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
    /// Show the neutral test card in the head window (plumbing check).
    dev_test_card: bool,
    /// Reveal the head and cycle through every visual state.
    dev_states: bool,
    /// `--perf[=<idle seconds>]`: print `perf:` lines, then exit after the
    /// idle sample.
    perf: Option<Duration>,
    /// `--set-key <name>`: store an API key, then exit (spec §5.7).
    set_key: Option<String>,
    /// Do one thing with the shell, then exit (see `sohi_tools::helper`).
    helper: Option<Helper>,
}

/// What a helper process is for.
#[derive(Debug, PartialEq, Eq)]
enum Helper {
    Open(String),
    List,
    Media(String),
}

impl Helper {
    fn run(&self) -> ExitCode {
        let worked = match self {
            Self::Open(target) => helper::run_open(target),
            Self::List => helper::run_list(),
            Self::Media(action) => return ExitCode::from(helper::run_media(action)),
        };
        if worked {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> (Self, Vec<String>) {
        let mut parsed = Self::default();
        let mut unknown = Vec::new();
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                helper::OPEN => parsed.helper = args.next().map(Helper::Open),
                helper::MEDIA => parsed.helper = args.next().map(Helper::Media),
                helper::LIST => parsed.helper = Some(Helper::List),
                "--set-key" => parsed.set_key = Some(args.next().unwrap_or_default()),
                "--test-panic" => parsed.test_panic = true,
                "--dev-test-card" => parsed.dev_test_card = true,
                "--dev-states" => parsed.dev_states = true,
                "--perf" => parsed.perf = Some(perf::DEFAULT_IDLE),
                _ => match arg.strip_prefix("--perf=").map(str::parse) {
                    Some(Ok(secs)) => parsed.perf = Some(Duration::from_secs(secs)),
                    _ => unknown.push(arg),
                },
            }
        }
        (parsed, unknown)
    }
}

/// `name`, or (with the `dev-inject` feature and `SOHI_DEV_INSTANCE` set)
/// one of its own, so a test build runs beside the Sohi in daily use.
fn instance_name(name: &str) -> String {
    #[cfg(feature = "dev-inject")]
    if let Ok(instance) = std::env::var("SOHI_DEV_INSTANCE") {
        return format!("{name}.{instance}");
    }
    name.to_owned()
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
    let (args, unknown) = Args::parse(std::env::args().skip(1));
    if let Some(helper) = &args.helper {
        return Ok(helper.run());
    }
    if let Some(name) = &args.set_key {
        // A one-off action: no window, no log and no single-instance check,
        // so it also works while Sohi is running.
        return Ok(set_key::run(name));
    }
    let window_class = instance_name(APP_WINDOW_CLASS);
    let wake_message = sohi_win::register_window_message(WAKE_MESSAGE)?;
    let Some(_instance) = SingleInstance::acquire(&instance_name(INSTANCE_MUTEX))? else {
        // Another instance owns the mutex: ask it to wake and leave at once.
        // The second process never opens the log, so rotation has one owner.
        sohi_win::forward_to_running_instance(&window_class, wake_message)?;
        return Ok(ExitCode::SUCCESS);
    };

    // Logging is best effort: Sohi still runs if the log dir is unwritable.
    let _ = logger::init(&logger::default_dir(), log::LevelFilter::Info);
    logger::install_panic_hook();
    log::info!("sohi {} starting", env!("CARGO_PKG_VERSION"));

    let config = config_file::load(&config_file::default_path());
    log::set_max_level(config_file::level_filter(config.log.level));

    if !unknown.is_empty() {
        log::warn!("ignoring unknown arguments: {unknown:?}");
    }
    assert!(!args.test_panic, "--test-panic requested");

    let queue = EventQueue::new();
    let window = AppWindow::create(&window_class, wake_message, queue.clone())?;
    let timer = Rc::new(FrameTimer::new()?);
    let paths = app::Paths {
        config: config_file::default_path(),
        logs: logger::default_dir(),
        offline_flag: config_file::default_path().with_file_name(offline_flag::NAME),
        reminders: config_file::default_path().with_file_name("reminders.json"),
    };
    let options = app::Options {
        reduced_motion: config.ui.reduced_motion,
        perf: args.perf.is_some(),
        wake: config.wake.clone(),
        llm_model: config.llm.model.clone(),
        offline_model: config.llm.offline_model.clone(),
        llm_fallback_model: config.llm.anthropic_fallback().to_owned(),
        user_name: config.persona.user_name.clone(),
        stt: config.stt.clone(),
        tts: config.tts.clone(),
        tools: config.tools.clone(),
        speak_reminders: config.reminders.speak,
        sounds: config.sounds.clone(),
    };
    let mut app = app::App::new(&queue, Rc::clone(&timer), &window, paths, &options)?;
    if args.dev_test_card {
        app.head().show_test_card()?;
    } else if args.dev_states {
        app.head().start_dev_cycle();
    } else if config.ui.launch_peek {
        app.peek(sohi_core::motion::LAUNCH_PEEK_HOLD);
    }

    // Armed: every startup step is done (spec §5.8 step 4).
    let _armed_event = match args.perf {
        Some(idle) => Some(perf::on_armed(window.remote(), idle)?),
        None => None,
    };
    let code = sohi_win::run_message_loop(&queue, &timer, |event| app.handle(event));
    log::info!("exiting with code {code}");
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> (Args, Vec<String>) {
        Args::parse(args.iter().map(|&a| a.to_owned()))
    }

    #[test]
    fn set_key_takes_the_next_argument() {
        let (args, unknown) = parse(&["--set-key", "mistral", "--bogus"]);
        assert_eq!(args.set_key.as_deref(), Some("mistral"));
        assert_eq!(unknown, ["--bogus"]);
        let (args, _) = parse(&["--set-key"]);
        assert_eq!(args.set_key.as_deref(), Some(""));
    }

    #[test]
    fn perf_takes_an_optional_duration() {
        assert_eq!(parse(&["--perf=5"]).0.perf, Some(Duration::from_secs(5)));
        assert_eq!(parse(&["--perf"]).0.perf, Some(perf::DEFAULT_IDLE));
        assert_eq!(parse(&["--perf=x"]).1, ["--perf=x"]);
    }
}
