//! UI-thread application state: owns the windows and reacts to events.

use std::rc::Rc;
use std::time::Instant;

use sohi_core::geometry::{RectPx, head_window_rect};
use sohi_win::{EventQueue, FRAME_30_FPS, Frame, FrameTimer, HeadWindow, WinError, WinEvent};

use crate::dev::TestCardPulse;

/// Everything the UI thread owns.
pub struct App {
    head: HeadWindow,
    /// Shared with the message loop; ticks only while something animates.
    timer: Rc<FrameTimer>,
    /// Current head window rectangle in physical pixels.
    head_rect: RectPx,
    /// Whether the pointer is over the head (pauses auto-retract from P2).
    hovered: bool,
    /// Running `--dev-test-card` pulse, if any.
    test_card: Option<TestCardPulse>,
}

impl App {
    /// Creates the (hidden) head window and places it on the primary monitor.
    ///
    /// # Errors
    /// Returns [`WinError`] if a window cannot be created.
    pub fn new(queue: &EventQueue, timer: Rc<FrameTimer>) -> Result<Self, WinError> {
        let head = HeadWindow::create(queue.clone())?;
        let mut app = Self {
            head,
            timer,
            head_rect: RectPx::default(),
            hovered: false,
            test_card: None,
        };
        app.relayout();
        Ok(app)
    }

    /// Handles one window event.
    pub fn handle(&mut self, event: WinEvent) {
        match event {
            WinEvent::WakeRequested => log::info!("wake requested"),
            WinEvent::Frame => self.on_frame(),
            WinEvent::HeadClicked => log::info!("head clicked"),
            WinEvent::HeadContextMenu { x, y } => log::info!("head context menu at ({x}, {y})"),
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
        self.test_card = Some(TestCardPulse::new(Instant::now()));
        self.timer.start(FRAME_30_FPS);
        Ok(())
    }

    fn on_frame(&mut self) {
        let Some(pulse) = self.test_card.as_ref() else {
            // Nothing animates: a stray tick must not keep the timer alive.
            self.timer.stop();
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

    fn draw_test_card(&mut self, level: u8) -> Result<(), WinError> {
        let (w, h) = (self.head_rect.width(), self.head_rect.height());
        let pixels = sohi_render::test_card(w, h, level);
        let frame = Frame {
            width: w,
            height: h,
            rgba: &pixels,
        };
        self.head
            .present(frame, self.head_rect.left, self.head_rect.top, 255)
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
        let rect = head_window_rect(monitor.work, monitor.dpi);
        if rect != self.head_rect {
            log::info!(
                "head at {rect:?} (work {:?}, dpi {})",
                monitor.work,
                monitor.dpi
            );
            self.head.set_rect(rect);
            self.head_rect = rect;
        }
    }
}
