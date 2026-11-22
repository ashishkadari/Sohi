//! UI-thread application state: owns the windows and reacts to events.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use sohi_core::config::ReducedMotion;
use sohi_core::geometry::{BASE_DPI, RectPx, head_window_rect};
use sohi_core::motion::{LAUNCH_PEEK_HOLD, Pose, Slide};
use sohi_core::visual::VisualState;
use sohi_render::{HeadRenderer, Scene};
use sohi_win::shell::{OpenError, open_path, open_with};
use sohi_win::{
    AppWindow, EventQueue, FRAME_30_FPS, FRAME_60_FPS, Frame, FrameTimer, HeadWindow, RemoteWindow,
    TrayIcon, WinError, WinEvent,
};

use crate::dev::{TestCardPulse, percentile};
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
}

/// A timer tick this close to a deadline counts as reaching it.
const TICK_SLACK: Duration = Duration::from_millis(8);

/// Where the head is in its reveal/retract cycle (spec §4.3). The full
/// visibility state machine with auto-retract rules arrives in P2.
#[derive(Debug, Clone, Copy)]
enum Visibility {
    /// Hidden; nothing renders.
    Parked,
    /// Moving; `hold` is how long to stay shown after a reveal.
    Sliding {
        slide: Slide,
        start: Instant,
        hold: Duration,
    },
    /// Fully shown until `retract_at`.
    Shown { retract_at: Instant },
}

/// Everything the UI thread owns.
pub struct App {
    head: HeadWindow,
    tray: TrayIcon,
    /// For quitting: closing the app window ends the message loop.
    main_window: RemoteWindow,
    paths: Paths,
    menu_state: MenuState,
    /// DPI the head and tray icon were last laid out for.
    dpi: u32,
    /// Shared with the message loop; ticks only while something animates.
    timer: Rc<FrameTimer>,
    /// Current head window rectangle in physical pixels.
    head_rect: RectPx,
    /// Whether the pointer is over the head (pauses auto-retract from P2).
    hovered: bool,
    /// Running `--dev-test-card` pulse, if any.
    test_card: Option<TestCardPulse>,
    /// Whether the static test card is on screen (redrawn on DPI change).
    showing_test_card: bool,
    visibility: Visibility,
    /// The user's reduced-motion preference.
    reduced_pref: ReducedMotion,
    /// Reduced motion, decided when the current reveal started.
    reduced: bool,
    /// Draws the head; recreated when the window size changes.
    renderer: Option<HeadRenderer>,
    /// Perf mode only.
    perf: Option<PerfTrace>,
}

/// `--perf` bookkeeping for head frames.
#[derive(Default)]
struct PerfTrace {
    /// The first visible frame has been reported.
    first_frame_done: bool,
    /// Present cost of each frame since the last report.
    costs: Vec<Duration>,
}

impl App {
    /// Creates the (hidden) head window, places it on the primary monitor
    /// and adds the tray icon.
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
        let head = HeadWindow::create(queue.clone())?;
        let dpi = sohi_win::primary_monitor().map_or(BASE_DPI, |m| m.dpi);
        if !sohi_win::force_dark_menus() {
            log::warn!("dark menus unavailable; using the system theme");
        }
        let tray = TrayIcon::add(window, dpi, sohi_render::tray_icon)?;
        let mut app = Self {
            head,
            tray,
            main_window: window.remote(),
            paths,
            menu_state: MenuState::default(),
            dpi,
            timer,
            head_rect: RectPx::default(),
            hovered: false,
            test_card: None,
            showing_test_card: false,
            visibility: Visibility::Parked,
            reduced_pref: options.reduced_motion,
            reduced: false,
            renderer: None,
            perf: options.perf.then(PerfTrace::default),
        };
        app.relayout();
        Ok(app)
    }

    /// Handles one window event.
    pub fn handle(&mut self, event: WinEvent) {
        match event {
            WinEvent::WakeRequested => {
                // Until the P2 wake logic exists, a wake is a short peek.
                log::info!("wake requested");
                self.peek(LAUNCH_PEEK_HOLD);
            }
            WinEvent::Frame => self.on_frame(),
            WinEvent::DisplayChanged => self.on_display_changed(),
            WinEvent::HeadClicked => log::info!("head clicked"),
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
            WinEvent::HeadHover(over) => {
                log::debug!("head hover: {over}");
                self.hovered = over;
            }
        }
    }

    /// Shows the neutral test card and pulses it at 30 fps for a few
    /// seconds (`--dev-test-card`), then leaves it static.
    ///
    /// # Errors
    /// Returns [`WinError`] if the first frame cannot be presented.
    pub fn show_test_card(&mut self) -> Result<(), WinError> {
        self.draw_test_card(0)?;
        self.head.show();
        self.showing_test_card = true;
        self.test_card = Some(TestCardPulse::new(Instant::now()));
        self.timer.start(FRAME_30_FPS);
        Ok(())
    }

    /// Slides the head in, keeps it shown for `hold`, then slides it out.
    /// While shown, another peek extends the hold; while sliding it is
    /// ignored.
    pub fn peek(&mut self, hold: Duration) {
        let now = Instant::now();
        match self.visibility {
            Visibility::Parked => {
                self.reduced = match self.reduced_pref {
                    ReducedMotion::System => !sohi_win::animations_enabled(),
                    ReducedMotion::On => true,
                    ReducedMotion::Off => false,
                };
                log::info!("reveal (reduced motion: {})", self.reduced);
                self.visibility = Visibility::Sliding {
                    slide: Slide::Reveal,
                    start: now,
                    hold,
                };
                // Draw the clear first pose before showing, so no stale
                // pixels flash, then tick at 60 fps.
                let first = Slide::Reveal
                    .pose(Duration::ZERO, self.reduced)
                    .unwrap_or(Pose::SHOWN);
                self.present_pose(first);
                self.head.show();
                self.timer.start(FRAME_60_FPS);
            }
            Visibility::Shown { .. } => {
                self.visibility = Visibility::Shown {
                    retract_at: now + hold,
                };
                self.timer.start(hold);
            }
            Visibility::Sliding { .. } => {}
        }
    }

    fn step_visibility(&mut self, now: Instant) {
        match self.visibility {
            Visibility::Parked => self.timer.stop(),
            Visibility::Sliding { slide, start, hold } => {
                match slide.pose(now.saturating_duration_since(start), self.reduced) {
                    Some(pose) => self.present_pose(pose),
                    None if slide == Slide::Reveal => {
                        self.present_pose(Pose::SHOWN);
                        // Static until the hold ends: one tick at the deadline.
                        self.visibility = Visibility::Shown {
                            retract_at: now + hold,
                        };
                        self.timer.start(hold);
                    }
                    None => {
                        self.head.hide();
                        self.visibility = Visibility::Parked;
                        self.timer.stop();
                        self.report_frame_costs();
                        log::info!("parked");
                    }
                }
            }
            Visibility::Shown { retract_at } => {
                if now + TICK_SLACK >= retract_at {
                    self.visibility = Visibility::Sliding {
                        slide: Slide::Retract,
                        start: now,
                        hold: Duration::ZERO,
                    };
                    self.timer.start(FRAME_60_FPS);
                } else {
                    self.timer.start(retract_at - now);
                }
            }
        }
    }

    /// Presents the head pixels at `pose`, regenerating them if the window
    /// size changed.
    fn present_pose(&mut self, pose: Pose) {
        let started = Instant::now();
        let (w, h) = (self.head_rect.width(), self.head_rect.height());
        if self.renderer.as_ref().map(HeadRenderer::size) != Some((w, h)) {
            self.renderer = HeadRenderer::new(w, h);
        }
        let Some(renderer) = self.renderer.as_mut() else {
            log::warn!("cannot draw a {w}x{h} head");
            return;
        };
        let frame = Frame {
            width: w,
            height: h,
            rgba: renderer.render(&Scene::new(VisualState::Idle, Duration::ZERO)),
        };
        let alpha = pose.alpha_u8();
        let result = self.head.present(
            frame,
            self.head_rect.left,
            self.head_rect.top,
            alpha,
            pose.shift_px(w),
        );
        if let Err(err) = result {
            log::warn!("head frame failed: {err}");
            return;
        }
        if let Some(trace) = self.perf.as_mut() {
            trace.costs.push(started.elapsed());
            if alpha > 0 && !trace.first_frame_done {
                trace.first_frame_done = true;
                crate::perf::on_first_frame();
            }
        }
    }

    /// Perf mode: logs the present cost of the frames since the last report.
    fn report_frame_costs(&mut self) {
        let Some(costs) = self.perf.as_mut().map(|t| &mut t.costs) else {
            return;
        };
        if costs.is_empty() {
            return;
        }
        costs.sort_unstable();
        crate::perf::on_slide_frames(costs.len(), percentile(costs, 95), costs[costs.len() - 1]);
        costs.clear();
    }

    fn on_frame(&mut self) {
        let Some(pulse) = self.test_card.as_ref() else {
            self.step_visibility(Instant::now());
            return;
        };
        let Some(level) = pulse.level(Instant::now()) else {
            // Static from here on: the last frame stays, with zero redraws.
            self.timer.stop();
            if let Some(pulse) = self.test_card.take() {
                log::info!("test card pulse done: {}", pulse.report());
            }
            return;
        };
        let started = Instant::now();
        if let Err(err) = self.draw_test_card(level) {
            log::warn!("test card frame failed: {err}");
        }
        if let Some(pulse) = self.test_card.as_mut() {
            pulse.record(started.elapsed());
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
            MenuCommand::Talk
            | MenuCommand::Type
            | MenuCommand::ForceOffline
            | MenuCommand::StartWithWindows => {
                log::info!("{command:?} is not available yet");
            }
        }
    }

    fn on_display_changed(&mut self) {
        log::info!("display changed");
        let moved = self.relayout();
        if moved && matches!(self.visibility, Visibility::Shown { .. }) {
            // Static content: redraw once at the new size and position.
            self.present_pose(Pose::SHOWN);
        }
        if moved && self.showing_test_card && self.test_card.is_none() {
            // Static content: redraw once at the new size and position.
            if let Err(err) = self.draw_test_card(128) {
                log::warn!("test card redraw failed: {err}");
            }
        }
    }

    fn draw_test_card(&mut self, level: u8) -> Result<(), WinError> {
        let (w, h) = (self.head_rect.width(), self.head_rect.height());
        let pixels = sohi_render::test_card(w, h, level);
        let frame = Frame {
            width: w,
            height: h,
            rgba: &pixels,
        };
        self.head
            .present(frame, self.head_rect.left, self.head_rect.top, 255, 0)
    }

    /// Recomputes the head position from the primary monitor's work area.
    /// Returns `true` if it moved or changed size.
    fn relayout(&mut self) -> bool {
        let monitor = match sohi_win::primary_monitor() {
            Ok(m) => m,
            Err(err) => {
                log::warn!("cannot read primary monitor: {err}");
                return false;
            }
        };
        if monitor.dpi != self.dpi {
            self.dpi = monitor.dpi;
            if let Err(err) = self.tray.set_dpi(monitor.dpi, sohi_render::tray_icon) {
                log::warn!("cannot resize tray icon: {err}");
            }
        }
        let rect = head_window_rect(monitor.work, monitor.dpi);
        if rect != self.head_rect {
            log::info!(
                "head at {rect:?} (work {:?}, dpi {})",
                monitor.work,
                monitor.dpi
            );
            self.head.set_rect(rect);
            self.head_rect = rect;
            return true;
        }
        false
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
