//! UI-thread application state: owns the windows and reacts to events.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use sohi_core::config::{ReducedMotion, SttConfig, ToolsConfig, TtsConfig, WakeConfig};
use sohi_core::connectivity::Event as NetEvent;
use sohi_core::geometry::{BASE_DPI, RectPx, head_window_rect};
use sohi_core::interaction::Event as TextEvent;
use sohi_core::visibility::Input;
use sohi_core::wake::CornerSettings;
use sohi_tools::Agenda;
use sohi_win::{
    AppTimer, AppWindow, EventQueue, FrameTimer, HotkeyHost, NetworkWatch, Presence, PresenceWatch,
    RemoteWindow, TimerHost, TrayIcon, WinError, WinEvent,
};

use crate::corner::Corner;
use crate::head::Head;
use crate::hotkeys::{self, Hotkeys};
use crate::offline_flag;
use crate::text_mode::{TextMode, TextSettings};
use crate::ticker::Ticker;
use crate::tray_menu::{self, MenuCommand, MenuState};

/// Files the menu can open.
pub struct Paths {
    /// `config.toml`.
    pub config: PathBuf,
    /// The log folder.
    pub logs: PathBuf,
    /// Exists while forced offline is on.
    pub offline_flag: PathBuf,
    /// The reminders.
    pub reminders: PathBuf,
}

/// Startup options that shape the app's behaviour.
pub struct Options {
    /// `[ui] reduced_motion` from the config.
    pub reduced_motion: ReducedMotion,
    /// `--perf`: report the first visible frame and slide frame costs.
    pub perf: bool,
    /// `[wake]`: hotkeys and corner tunables.
    pub wake: WakeConfig,
    /// `[llm] model`.
    pub llm_model: String,
    /// The Ollama model for offline turns; empty for none.
    pub offline_model: String,
    /// The Anthropic fallback model; empty for none.
    pub llm_fallback_model: String,
    /// `[persona] user_name`.
    pub user_name: String,
    /// `[stt]`.
    pub stt: SttConfig,
    /// `[tts]`.
    pub tts: TtsConfig,
    /// `[tools]`.
    pub tools: ToolsConfig,
    /// `[reminders] speak` from the config.
    pub speak_reminders: bool,
}

impl Options {
    fn text_settings(&self, forced_offline: bool, agenda: Agenda) -> TextSettings {
        TextSettings {
            model: self.llm_model.clone(),
            fallback_model: self.llm_fallback_model.clone(),
            offline_model: self.offline_model.clone(),
            user_name: self.user_name.clone(),
            stt: self.stt.clone(),
            tts: self.tts.clone(),
            tools: self.tools.clone(),
            follow_up: self.wake.follow_up,
            hotkey_text: self.wake.hotkey_text.clone(),
            speak_reminders: self.speak_reminders,
            agenda,
            forced_offline,
        }
    }
}

/// Name of the startup entry, as Settings shows it.
const AUTOSTART_NAME: &str = "Sohi";
/// Timer id, next to those of [`crate::text_mode`].
const TIMER_TRAY: usize = 4;
/// How long the taskbar gets before the tray icon is tried again.
const TRAY_RETRY: Duration = Duration::from_secs(3);
/// How often. Later, `TaskbarCreated` says when it is ready.
const MAX_TRAY_TRIES: u32 = 10;

/// Everything the UI thread owns.
pub struct App {
    head: Head,
    tray: TrayIcon,
    /// When the tray icon is tried again.
    tray_retry: Option<AppTimer>,
    /// How often that was done.
    tray_tries: u32,
    timers: TimerHost,
    /// For quitting: closing the app window ends the message loop.
    main_window: RemoteWindow,
    paths: Paths,
    menu_state: MenuState,
    /// DPI the head and tray icon were last laid out for.
    dpi: u32,
    /// Primary monitor bounds (`rcMonitor`), for the wake hot zone.
    monitor: RectPx,
    /// Corner wake polling.
    corner: Corner,
    /// No corner wake this soon after a retract.
    cooldown: Duration,
    /// Global hotkeys, unregistered on drop.
    _hotkeys: Hotkeys,
    /// Lock and display-power notifications, unregistered on drop.
    _presence: Option<PresenceWatch>,
    /// Connectivity hints, unsubscribed on drop.
    _network: Option<NetworkWatch>,
    /// The text pill and text turns.
    text: TextMode,
}

impl App {
    /// Creates the (hidden) head window, places it on the primary monitor,
    /// adds the tray icon and starts corner polling.
    ///
    /// # Errors
    /// Returns [`WinError`] if a window or the tray icon cannot be created.
    pub fn new(
        queue: &EventQueue,
        timer: Rc<FrameTimer>,
        window: &AppWindow,
        paths: Paths,
        options: &Options,
    ) -> Result<Self, WinError> {
        let ticker = Ticker::new(timer);
        let head = Head::new(
            queue,
            Rc::clone(&ticker),
            options.reduced_motion,
            options.perf,
        )?;
        let dpi = sohi_win::primary_monitor().map_or(BASE_DPI, |m| m.dpi);
        if !sohi_win::force_dark_menus() {
            log::warn!("dark menus unavailable; using the system theme");
        }
        let tray = TrayIcon::new(window, dpi, sohi_render::tray_icon)?;
        let hotkeys = Hotkeys::register(window, &tray, &options.wake);
        let corner = CornerSettings::from_config(&options.wake);
        let presence = PresenceWatch::register(window)
            .inspect_err(|err| log::warn!("lock/display notifications unavailable: {err}"))
            .ok();
        let forced_offline = paths.offline_flag.exists();
        let network = NetworkWatch::subscribe(window.remote())
            .inspect_err(|err| log::warn!("network hints unavailable: {err}"))
            .ok();
        let main_window = window.remote();
        let agenda = Agenda::open(paths.reminders.clone(), move || {
            if let Err(err) = main_window.post_runtime_event() {
                log::warn!("cannot report a changed reminder: {err}");
            }
        });
        let text = TextMode::new(
            queue.clone(),
            ticker,
            window.remote(),
            TimerHost::new(window),
            HotkeyHost::new(window),
            options.text_settings(forced_offline, agenda),
        );
        let mut app = Self {
            head,
            tray,
            tray_retry: None,
            tray_tries: 0,
            timers: TimerHost::new(window),
            main_window,
            paths,
            menu_state: MenuState {
                force_offline: forced_offline,
                ..MenuState::default()
            },
            dpi,
            monitor: RectPx::default(),
            corner: Corner::start(window, corner),
            cooldown: corner.cooldown,
            _hotkeys: hotkeys,
            _presence: presence,
            _network: network,
            text,
        };
        app.show_tray();
        app.relayout();
        app.text.show_base(&mut app.head);
        Ok(app)
    }

    /// Handles one window event.
    pub fn handle(&mut self, event: WinEvent) {
        self.dispatch(event);
        // A retract rule, the voice hotkey or a lock may have put the head
        // away: text mode closes with it.
        self.text.follow_head(&mut self.head);
        if let Some(text) = self.text.take_toast()
            && let Err(err) = self.tray.inform("Sohi", &text)
        {
            log::warn!("cannot show a toast: {err}");
        }
    }

    fn dispatch(&mut self, event: WinEvent) {
        match event {
            WinEvent::WakeRequested => {
                // A second launch: an explicit request, like a hotkey, so
                // full-screen suppression does not apply (spec §5.8).
                log::info!("wake requested");
                self.wake(false);
            }
            WinEvent::Frame => self.on_frame(),
            WinEvent::CursorPoll => self.on_cursor_poll(),
            WinEvent::Hotkey { id } => self.on_hotkey(id),
            WinEvent::Presence(presence) => self.on_presence(presence),
            WinEvent::DisplayChanged => {
                log::info!("display changed");
                self.relayout();
            }
            WinEvent::HeadClicked => {
                log::info!("head clicked");
                self.text.wake_text(TextEvent::HeadClicked, &mut self.head);
            }
            WinEvent::HeadContextMenu { x, y } | WinEvent::TrayMenu { x, y } => {
                self.show_menu(x, y);
            }
            WinEvent::TrayActivated => log::info!("tray icon activated"),
            WinEvent::TaskbarCreated => {
                log::info!("taskbar restarted; re-adding tray icon");
                self.tray_tries = 0;
                self.show_tray();
            }
            WinEvent::Runtime => {
                self.text.on_runtime(&mut self.head);
                // A tool may have changed them.
                self.text.arm_reminders();
            }
            WinEvent::PillSubmit
            | WinEvent::PillEscape
            | WinEvent::PillRecall
            | WinEvent::PillFocus(_) => self.text.on_pill(event, &mut self.head),
            WinEvent::Timer { id: TIMER_TRAY } => {
                self.tray_retry = None;
                self.show_tray();
            }
            WinEvent::Timer { id } => {
                if !self.text.on_timer(id, &mut self.head) {
                    log::debug!("unknown timer {id}");
                }
            }
            WinEvent::NetworkHint { internet } => {
                log::info!("network hint: internet {internet}");
                self.text.net(NetEvent::Hint { internet }, &mut self.head);
            }
            WinEvent::Full(event) => self.text.on_full(event, &mut self.head),
            WinEvent::HeadHover(over) => {
                log::debug!("head hover: {over}");
                self.head.input(Input::Hover(over));
            }
        }
    }

    fn on_presence(&mut self, presence: Presence) {
        log::info!("presence: {presence:?}");
        if presence == Presence::Suspended(false) {
            self.text.net(NetEvent::Resumed, &mut self.head);
        }
        if self.corner.on_presence(presence) {
            // Nobody is looking: put the head away too.
            self.head.input(Input::Retract);
            self.text.away(&mut self.head);
        }
        self.text.set_away(self.corner.is_locked(), &mut self.head);
    }

    fn on_frame(&mut self) {
        self.head.on_frame();
        self.text.full_frame(&mut self.head);
    }

    /// The head, for startup dev modes.
    pub fn head(&mut self) -> &mut Head {
        &mut self.head
    }

    /// Slides the head in and keeps it shown for `hold` (another peek while
    /// shown extends it).
    pub fn peek(&mut self, hold: Duration) {
        self.head.input(Input::Reveal {
            retract_after: Some(hold),
        });
    }

    /// Shows the tray icon. While the taskbar is busy (right after signing
    /// in) that can fail, so it is tried a few more times.
    fn show_tray(&mut self) {
        let Err(err) = self.tray.show() else {
            return;
        };
        log::warn!("cannot add the tray icon: {err}");
        if self.tray_tries < MAX_TRAY_TRIES {
            self.tray_tries += 1;
            self.tray_retry = self
                .timers
                .start(TIMER_TRAY, TRAY_RETRY)
                .inspect_err(|err| log::warn!("cannot start a timer: {err}"))
                .ok();
        }
    }

    /// A voice wake: reveal the head and listen (spec §3.3), or open the
    /// pill if offline; `focus` says whether the pill may take the
    /// keyboard.
    fn wake(&mut self, focus: bool) {
        self.text.wake(focus, &mut self.head);
    }

    fn on_cursor_poll(&mut self) {
        let (head, cooldown) = (&self.head, self.cooldown);
        if self
            .corner
            .on_poll(self.monitor, |now: Instant| head.corner_gate(now, cooldown))
        {
            log::info!("wake: corner");
            self.wake(false);
        }
    }

    /// The voice hotkey starts listening, stops it, or barges in on a
    /// reply (spec §5.2). Neither hotkey is affected by full-screen
    /// suppression.
    fn on_hotkey(&mut self, id: u16) {
        match id {
            hotkeys::VOICE => {
                log::info!("hotkey: voice");
                self.wake(true);
            }
            hotkeys::TEXT => {
                log::info!("hotkey: text");
                self.text.wake_text(TextEvent::TextHotkey, &mut self.head);
            }
            hotkeys::ESCAPE => {
                log::info!("hotkey: Esc");
                self.text.event(TextEvent::Escape, &mut self.head);
            }
            other => log::warn!("unknown hotkey id {other}"),
        }
    }

    /// Toggles forced offline and remembers it across restarts.
    fn toggle_offline(&mut self) {
        let forced = self.text.toggle_offline(&mut self.head);
        self.menu_state.force_offline = forced;
        if let Err(err) = offline_flag::set(&self.paths.offline_flag, forced) {
            log::warn!("cannot remember forced offline: {err}");
        }
    }

    fn show_menu(&mut self, x: i32, y: i32) {
        // It can be changed in Settings as well.
        self.menu_state.start_with_windows = std::env::current_exe()
            .is_ok_and(|exe| sohi_win::starts_with_windows(AUTOSTART_NAME, &exe));
        self.menu_state.full_mode = self.text.is_full();
        let items = tray_menu::items(self.menu_state);
        let chosen = self.tray.show_menu(&items, x, y);
        let Some(command) = chosen.and_then(MenuCommand::from_id) else {
            return;
        };
        log::info!("menu: {command:?}");
        match command {
            MenuCommand::OpenConfig => open(&self.paths.config),
            MenuCommand::OpenLogFolder => open(&self.paths.logs),
            MenuCommand::Quit => {
                if let Err(err) = self.main_window.post_close() {
                    log::error!("cannot quit cleanly: {err}");
                    sohi_win::post_quit(0);
                }
            }
            MenuCommand::Type => self.text.wake_text(TextEvent::HeadClicked, &mut self.head),
            MenuCommand::Talk => self.wake(true),
            MenuCommand::FullMode => self.text.toggle_full(&mut self.head),
            MenuCommand::ForceOffline => self.toggle_offline(),
            MenuCommand::StartWithWindows => {
                toggle_autostart(self.menu_state.start_with_windows);
            }
        }
    }

    /// Recomputes the head position from the primary monitor's work area.
    fn relayout(&mut self) {
        let monitor = match sohi_win::primary_monitor() {
            Ok(m) => m,
            Err(err) => {
                log::warn!("cannot read primary monitor: {err}");
                return;
            }
        };
        if monitor.dpi != self.dpi {
            self.dpi = monitor.dpi;
            if let Err(err) = self.tray.set_dpi(monitor.dpi, sohi_render::tray_icon) {
                log::warn!("cannot resize tray icon: {err}");
            }
        }
        self.monitor = monitor.monitor;
        let rect = head_window_rect(monitor.work, monitor.dpi);
        self.text.set_layout(rect, monitor);
        if self.head.set_rect(rect) {
            log::info!(
                "head at {rect:?} (work {:?}, dpi {})",
                monitor.work,
                monitor.dpi
            );
        }
    }
}

/// Has Windows start Sohi at sign-in, or no longer.
fn toggle_autostart(on_now: bool) {
    let result = std::env::current_exe()
        .map_err(|err| err.to_string())
        .and_then(|exe| {
            sohi_win::set_starts_with_windows(AUTOSTART_NAME, &exe, !on_now)
                .map_err(|err| err.to_string())
        });
    match result {
        Ok(()) => log::info!("start with Windows: {}", !on_now),
        Err(err) => log::warn!("cannot change start with Windows: {err}"),
    }
}

/// Opens the config or the log folder.
fn open(path: &Path) {
    if let Err(err) = sohi_tools::helper::open_soon(path) {
        log::warn!("cannot open {}: {err}", path.display());
    }
}
