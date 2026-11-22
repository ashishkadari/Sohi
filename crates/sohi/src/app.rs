//! UI-thread application state: owns the windows and reacts to events.

use sohi_core::geometry::{RectPx, head_window_rect};
use sohi_win::{EventQueue, HeadWindow, WinError, WinEvent};

/// Everything the UI thread owns.
pub struct App {
    head: HeadWindow,
    /// Current head window rectangle in physical pixels.
    head_rect: RectPx,
    /// Whether the pointer is over the head (pauses auto-retract from P2).
    hovered: bool,
}

impl App {
    /// Creates the (hidden) head window and places it on the primary monitor.
    ///
    /// # Errors
    /// Returns [`WinError`] if a window cannot be created.
    pub fn new(queue: &EventQueue) -> Result<Self, WinError> {
        let head = HeadWindow::create(queue.clone())?;
        let mut app = Self {
            head,
            head_rect: RectPx::default(),
            hovered: false,
        };
        app.relayout();
        Ok(app)
    }

    /// Handles one window event.
    pub fn handle(&mut self, event: WinEvent) {
        match event {
            WinEvent::WakeRequested => log::info!("wake requested"),
            WinEvent::HeadClicked => log::debug!("head clicked"),
            WinEvent::HeadContextMenu { x, y } => log::debug!("head context menu at ({x}, {y})"),
            WinEvent::HeadHover(over) => {
                log::debug!("head hover: {over}");
                self.hovered = over;
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
