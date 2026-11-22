//! The runtime thread and its channels to and from the UI thread (spec
//! §3.2, §9.3).
//!
//! The thread runs a tokio current-thread runtime. It is started lazily,
//! the first time network work is needed, so a parked Sohi that was never
//! used has no second thread.
//!
//! - UI → runtime: a bounded tokio channel. The UI only ever `try_send`s,
//!   so it never blocks.
//! - Runtime → UI: a `std::sync::mpsc` queue, plus a posted message that
//!   wakes the UI's message loop ([`WinEvent::Runtime`]). The UI drains the
//!   whole queue each time.
//!
//! [`WinEvent::Runtime`]: sohi_win::WinEvent::Runtime

use std::future::Future;
use std::sync::mpsc;
use std::thread::JoinHandle;

use sohi_win::RemoteWindow;
use tokio::sync::mpsc as tokio_mpsc;

/// How many requests may wait for the runtime before `send` drops them.
pub const REQUEST_CAPACITY: usize = 64;

/// The runtime side's handle for sending updates to the UI thread.
pub struct UiLink<U> {
    tx: mpsc::Sender<U>,
    window: RemoteWindow,
}

impl<U> Clone for UiLink<U> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            window: self.window,
        }
    }
}

impl<U> UiLink<U> {
    /// Queues `update` and wakes the UI thread. Returns `false` if the UI
    /// is gone (the app is shutting down).
    pub fn send(&self, update: U) -> bool {
        if self.tx.send(update).is_err() {
            return false;
        }
        // A failed post means the window is gone. The update stays queued;
        // nobody will read it, which is fine during shutdown.
        self.window.post_runtime_event().is_ok()
    }
}

/// Why a request did not reach the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// [`REQUEST_CAPACITY`] requests are already waiting.
    Full,
    /// The runtime thread has stopped.
    Stopped,
}

/// The UI thread's handle on the runtime thread. Dropping it closes the
/// request channel and waits for the thread to finish.
pub struct Runtime<R, U> {
    requests: Option<tokio_mpsc::Sender<R>>,
    updates: mpsc::Receiver<U>,
    thread: Option<JoinHandle<()>>,
}

impl<R: Send + 'static, U: Send + 'static> Runtime<R, U> {
    /// Starts the thread and runs `serve` on it until `serve` returns.
    ///
    /// `serve` gets the request receiver and a [`UiLink`]. It should return
    /// once the receiver yields `None`, which happens when this handle is
    /// dropped.
    ///
    /// # Errors
    /// Returns the OS error if the thread cannot be spawned.
    pub fn start<F, Fut>(window: RemoteWindow, serve: F) -> std::io::Result<Self>
    where
        F: FnOnce(tokio_mpsc::Receiver<R>, UiLink<U>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        let (request_tx, request_rx) = tokio_mpsc::channel(REQUEST_CAPACITY);
        let (update_tx, update_rx) = mpsc::channel();
        let link = UiLink {
            tx: update_tx,
            window,
        };
        let thread = std::thread::Builder::new()
            .name("sohi-runtime".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(serve(request_rx, link)),
                    Err(err) => log::error!("cannot build the async runtime: {err}"),
                }
            })?;
        log::info!("runtime thread started");
        Ok(Self {
            requests: Some(request_tx),
            updates: update_rx,
            thread: Some(thread),
        })
    }

    /// Hands `request` to the runtime without blocking.
    ///
    /// # Errors
    /// Returns [`SendError`] if the queue is full or the runtime stopped.
    pub fn send(&self, request: R) -> Result<(), SendError> {
        let Some(requests) = &self.requests else {
            return Err(SendError::Stopped);
        };
        requests.try_send(request).map_err(|err| match err {
            tokio_mpsc::error::TrySendError::Full(_) => SendError::Full,
            tokio_mpsc::error::TrySendError::Closed(_) => SendError::Stopped,
        })
    }

    /// Every update queued so far, oldest first. Never blocks.
    pub fn drain(&self) -> mpsc::TryIter<'_, U> {
        self.updates.try_iter()
    }
}

impl<R, U> Drop for Runtime<R, U> {
    fn drop(&mut self) {
        // Closing the channel tells `serve` to return.
        drop(self.requests.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("runtime thread panicked");
        }
        log::info!("runtime thread stopped");
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use sohi_win::{AppWindow, EventQueue};

    use super::*;

    /// Echoes every request back, doubled, until the channel closes.
    async fn echo(mut rx: tokio_mpsc::Receiver<u32>, ui: UiLink<u32>) {
        while let Some(n) = rx.recv().await {
            tokio::time::sleep(Duration::from_millis(1)).await;
            ui.send(n * 2);
        }
    }

    fn wait_for(runtime: &Runtime<u32, u32>, count: usize) -> Vec<u32> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < count && Instant::now() < deadline {
            got.extend(runtime.drain());
            std::thread::sleep(Duration::from_millis(2));
        }
        got
    }

    #[test]
    fn requests_round_trip_in_order() {
        let window = AppWindow::create("Sohi.Test.Runtime", 0xC124, EventQueue::new()).unwrap();
        let runtime = Runtime::start(window.remote(), echo).unwrap();
        for n in 1..=5 {
            runtime.send(n).unwrap();
        }
        assert_eq!(wait_for(&runtime, 5), [2, 4, 6, 8, 10]);
        drop(runtime); // must not hang: closing the channel ends `echo`
    }

    #[test]
    fn a_full_queue_refuses_instead_of_blocking() {
        let window = AppWindow::create("Sohi.Test.Runtime2", 0xC125, EventQueue::new()).unwrap();
        // Never reads, so the queue fills up.
        let runtime: Runtime<u32, u32> = Runtime::start(window.remote(), |rx, _ui| async move {
            let _keep_open = rx;
            std::future::pending::<()>().await;
        })
        .unwrap();
        for n in 0..REQUEST_CAPACITY {
            runtime.send(u32::try_from(n).unwrap()).unwrap();
        }
        assert_eq!(runtime.send(0), Err(SendError::Full));
        std::mem::forget(runtime); // its `serve` never returns
    }

    #[test]
    fn send_after_the_runtime_stops_reports_it() {
        let window = AppWindow::create("Sohi.Test.Runtime3", 0xC126, EventQueue::new()).unwrap();
        let runtime: Runtime<u32, u32> =
            Runtime::start(window.remote(), |_rx, _ui| async {}).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.send(1) != Err(SendError::Stopped) {
            assert!(Instant::now() < deadline, "runtime never stopped");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
