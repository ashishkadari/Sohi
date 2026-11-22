//! Events produced by Sohi's windows, and the queue that carries them.
//!
//! Window procedures never call into application code. They push a
//! [`WinEvent`] onto the shared [`EventQueue`], and the message loop hands
//! queued events to the app's handler after dispatching. That keeps app
//! state free of re-entrancy: a handler can call any `sohi-win` method,
//! even one that synchronously sends messages, without aliasing itself.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Something the UI thread needs to react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinEvent {
    /// Another launch asked this instance to wake.
    WakeRequested,
    /// The frame timer ticked: time to draw the next animation frame.
    Frame,
    /// Resolution, scaling, work area or primary monitor may have changed;
    /// placement must be recomputed.
    DisplayChanged,
    /// Left click released on the head.
    HeadClicked,
    /// Right click released on the head, at screen coordinates.
    HeadContextMenu {
        /// Screen x.
        x: i32,
        /// Screen y.
        y: i32,
    },
    /// The pointer entered (`true`) or left (`false`) the head's opaque pixels.
    HeadHover(bool),
    /// The tray icon was right-clicked (or menu key), at screen coordinates.
    TrayMenu {
        /// Screen x.
        x: i32,
        /// Screen y.
        y: i32,
    },
    /// The tray icon was left-clicked or activated from the keyboard.
    TrayActivated,
    /// Explorer (re)started: the tray icon must be added again.
    TaskbarCreated,
}

/// Shared, single-threaded FIFO of [`WinEvent`]s.
///
/// Cloning gives another handle to the same queue.
#[derive(Debug, Clone, Default)]
pub struct EventQueue(Rc<RefCell<VecDeque<WinEvent>>>);

impl EventQueue {
    /// Creates an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an event.
    pub fn push(&self, event: WinEvent) {
        self.0.borrow_mut().push_back(event);
    }

    /// Removes the oldest event. The borrow ends before this returns, so
    /// the caller may push while handling the event.
    #[must_use]
    pub fn pop(&self) -> Option<WinEvent> {
        self.0.borrow_mut().pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_and_shared_between_clones() {
        let a = EventQueue::new();
        let b = a.clone();
        a.push(WinEvent::WakeRequested);
        assert_eq!(b.pop(), Some(WinEvent::WakeRequested));
        assert_eq!(a.pop(), None);
    }

    #[test]
    fn can_push_while_handling() {
        let q = EventQueue::new();
        q.push(WinEvent::WakeRequested);
        while let Some(event) = q.pop() {
            if event == WinEvent::WakeRequested {
                // Would panic with "already borrowed" if pop held the borrow.
                q.push(WinEvent::WakeRequested);
                break;
            }
        }
        assert_eq!(q.pop(), Some(WinEvent::WakeRequested));
    }
}
