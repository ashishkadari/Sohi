//! UI-thread application state: owns the windows and reacts to events.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use sohi_core::config::{ReducedMotion, WakeConfig};
use sohi_core::geometry::{BASE_DPI, RectPx, head_window_rect};
use sohi_core::interaction::Event as TextEvent;
use sohi_core::visibility::{Input, Phase, RetractRule};
use sohi_core::wake::CornerSettings;
use sohi_win::shell::{OpenError, open_path, open_with};
use sohi_win::{
    AppWindow, EventQueue, FrameTimer, PresenceWatch, RemoteWindow, TimerHost, TrayIcon, WinError,
    WinEvent,
};

use crate::corner::Corner;
use crate::head::Head;
use crate::hotkeys::{self, Hotkeys};
use crate::text_mode::{TextMode, TextSettings};
use crate::tray_menu::{self, MenuCommand, MenuState};

/// Files the menu can open.
pub struct Paths {
    /// `config.toml`.
    pub config: PathBuf,
    /// The log folder.
    pub logs: PathBuf,
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
    /// `[persona] user_name`.
    pub user_name: String,
}

/// Everything the UI thread owns.
pub struct App {
    head: Head,
    tray: TrayIcon,
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
        let head = Head::new(queue, timer, options.reduced_motion, options.perf)?;
        let dpi = sohi_win::primary_monitor().map_or(BASE_DPI, |m| m.dpi);
        if !sohi_win::force_dark_menus() {
            log::warn!("dark menus unavailable; using the system theme");
        }
        let tray = TrayIcon::add(window, dpi, sohi_render::tray_icon)?;
        let hotkeys = Hotkeys::register(window, &tray, &options.wake);
        let corner = CornerSettings::from_config(&options.wake);
        let presence = PresenceWatch::register(window)
            .inspect_err(|err| log::warn!("lock/display notifications unavailable: {err}"))
            .ok();
        let text = TextMode::new(
            queue.clone(),
            window.remote(),
            TimerHost::new(window),
            TextSettings {
                model: options.llm_model.clone(),
                user_name: options.user_name.clone(),
            },
        );
        let mut app = Self {
            head,
            tray,
            main_window: window.remote(),
            paths,
            menu_state: MenuState::default(),
            dpi,
            monitor: RectPx::default(),
            corner: Corner::start(window, corner),
            cooldown: corner.cooldown,
            _hotkeys: hotkeys,
            _presence: presence,
            text,
        };
        app.relayout();
        Ok(app)
    }

    /// Handles one window event.
    pub fn handle(&mut self, event: WinEvent) {
        self.dispatch(event);
        // A retract rule, the voice hotkey or a lock may have put the head
        // away: text mode closes with it.
        self.text.follow_head(&mut self.head);
    }

    fn dispatch(&mut self, event: WinEvent) {
        match event {
            WinEvent::WakeRequested => {
                // A second launch: an explicit request, like a hotkey, so
                // full-screen suppression does not apply (spec §5.8).
                log::info!("wake requested");
                self.wake();
            }
            WinEvent::Frame => self.head.on_frame(),
            WinEvent::CursorPoll => self.on_cursor_poll(),
            WinEvent::Hotkey { id } => self.on_hotkey(id),
            WinEvent::Presence(presence) => {
                log::info!("presence: {presence:?}");
                if self.corner.on_presence(presence) {
                    // Nobody is looking: put the head away too.
                    self.head.input(Input::Retract);
                }
            }
            WinEvent::DisplayChanged => {
                log::info!("display changed");
                self.relayout();
            }
            WinEvent::HeadClicked => {
                log::info!("head clicked");
                self.text.event(TextEvent::HeadClicked, &mut self.head);
            }
            WinEvent::HeadContextMenu { x, y } | WinEvent::TrayMenu { x, y } => {
                self.show_menu(x, y);
            }
            WinEvent::TrayActivated => log::info!("tray icon activated"),
            WinEvent::TaskbarCreated => {
                log::info!("taskbar restarted; re-adding tray icon");
                if let Err(err) = self.tray.readd() {
                    log::warn!("cannot re-add tray icon: {err}");
                }
            }
            WinEvent::Runtime => self.text.on_runtime(&mut self.head),
            WinEvent::PillSubmit
            | WinEvent::PillEscape
            | WinEvent::PillRecall
            | WinEvent::PillFocus(_) => self.text.on_pill(event, &mut self.head),
            WinEvent::Timer { id } => {
                if !self.text.on_timer(id, &mut self.head) {
                    log::debug!("unknown timer {id}");
                }
            }
            WinEvent::HeadHover(over) => {
                log::debug!("head hover: {over}");
                self.head.input(Input::Hover(over));
            }
        }
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

    /// Reveals the head for a wake. Until voice exists (P3) nothing is
    /// said, so the no-speech rule retracts it (spec §4.4).
    fn wake(&mut self) {
        self.head.input(RetractRule::NoSpeech.reveal());
    }

    fn on_cursor_poll(&mut self) {
        let (head, cooldown) = (&self.head, self.cooldown);
        if self
            .corner
            .on_poll(self.monitor, |now: Instant| head.corner_gate(now, cooldown))
        {
            log::info!("wake: corner");
            self.wake();
        }
    }

    /// The voice hotkey toggles: it wakes a parked (or retracting) head and
    /// retracts a shown one. Until text mode exists the text hotkey only
    /// wakes. Neither is affected by full-screen suppression.
    fn on_hotkey(&mut self, id: u16) {
        let shown = matches!(self.head.phase(), Phase::Revealing | Phase::Shown);
        match id {
            hotkeys::VOICE if shown => {
                log::info!("hotkey: voice (retract)");
                self.head.input(Input::Retract);
            }
            hotkeys::VOICE => {
                log::info!("wake: hotkey (voice)");
                self.wake();
            }
            hotkeys::TEXT => {
                log::info!("hotkey: text");
                self.text.event(TextEvent::TextHotkey, &mut self.head);
            }
            other => log::warn!("unknown hotkey id {other}"),
        }
    }

    fn show_menu(&mut self, x: i32, y: i32) {
        let items = tray_menu::items(self.menu_state);
        let chosen = self.tray.show_menu(&items, x, y);
        let Some(command) = chosen.and_then(MenuCommand::from_id) else {
            return;
        };
        log::info!("menu: {command:?}");
        match command {
            MenuCommand::OpenConfig => open_config(&self.paths.config),
            MenuCommand::OpenLogFolder => {
                if let Err(err) = open_path(&self.paths.logs) {
                    log::warn!("cannot open log folder: {err:?}");
                }
            }
            MenuCommand::Quit => {
                if let Err(err) = self.main_window.post_close() {
                    log::error!("cannot quit cleanly: {err}");
                    sohi_win::post_quit(0);
                }
            }
            MenuCommand::Type => self.text.event(TextEvent::HeadClicked, &mut self.head),
            MenuCommand::Talk | MenuCommand::ForceOffline | MenuCommand::StartWithWindows => {
                log::info!("{command:?} is not available yet");
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
        self.text.set_layout(rect, monitor.dpi);
        if self.head.set_rect(rect) {
            log::info!(
                "head at {rect:?} (work {:?}, dpi {})",
                monitor.work,
                monitor.dpi
            );
        }
    }
}

/// Opens the config in its associated editor, or Notepad if `.toml` has
/// no association (common on a fresh Windows install).
fn open_config(path: &Path) {
    let result = match open_path(path) {
        Err(OpenError::NoAssociation) => open_with("notepad.exe", path),
        other => other,
    };
    if let Err(err) = result {
        log::warn!("cannot open config: {err:?}");
    }
}
