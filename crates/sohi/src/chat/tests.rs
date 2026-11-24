use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sohi_win::{AppWindow, EventQueue};

use super::*;
use crate::runtime::Runtime;

/// Sends one delta, then hangs until dropped.
struct Hanging {
    dropped: Arc<AtomicBool>,
}

struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl TurnRunner for Hanging {
    async fn run(&mut self, job: Job, ui: &UiLink<Update>) {
        let _flag = SetOnDrop(Arc::clone(&self.dropped));
        ui.send(Update::Delta {
            turn: job.id(),
            text: "partial".into(),
        });
        std::future::pending::<()>().await;
    }
}

fn turn(id: u64) -> Request {
    Request::Chat(ChatTurn {
        turn: id,
        model: String::new(),
        fallback_model: String::new(),
        offline: None,
        system: String::new(),
        messages: Vec::new(),
        policy: Policy::default(),
    })
}

fn cancel(turn: u64) -> Request {
    Request::Cancel {
        turn,
        at: Instant::now(),
    }
}

fn start(class: &str) -> (AppWindow, Runtime<Request, Update>, Arc<AtomicBool>) {
    let window = AppWindow::create(class, 0xC130, EventQueue::new()).unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let runner = Hanging {
        dropped: Arc::clone(&dropped),
    };
    let runtime = Runtime::start(window.remote(), move |rx, ui| {
        serve_with(rx, ui, runner, |_, _: &UiLink<Update>| ())
    })
    .unwrap();
    (window, runtime, dropped)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) -> Duration {
    let start = Instant::now();
    while !done() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_micros(200));
    }
    start.elapsed()
}

fn deltas(runtime: &Runtime<Request, Update>) -> Vec<u64> {
    runtime
        .drain()
        .filter_map(|u| match u {
            Update::Delta { turn, .. } => Some(turn),
            _ => None,
        })
        .collect()
}

#[test]
fn cancel_drops_the_running_turn_within_30_ms() {
    let (_window, runtime, dropped) = start("Sohi.Test.Cancel1");
    runtime.send(turn(1)).unwrap();
    let mut seen = Vec::new();
    wait_until("first delta", || {
        seen.extend(deltas(&runtime));
        !seen.is_empty()
    });
    runtime.send(cancel(1)).unwrap();
    let took = wait_until("turn dropped", || dropped.load(Ordering::SeqCst));
    assert!(took < Duration::from_millis(30), "took {took:?}");
}

#[test]
fn a_stale_cancel_leaves_the_running_turn_alone() {
    let (_window, runtime, dropped) = start("Sohi.Test.Cancel2");
    runtime.send(turn(2)).unwrap();
    runtime.send(cancel(1)).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(!dropped.load(Ordering::SeqCst));
    runtime.send(cancel(2)).unwrap();
    wait_until("turn dropped", || dropped.load(Ordering::SeqCst));
}

#[test]
fn a_new_turn_replaces_the_running_one() {
    let (_window, runtime, dropped) = start("Sohi.Test.Cancel3");
    runtime.send(turn(1)).unwrap();
    runtime.send(turn(2)).unwrap();
    let mut seen = Vec::new();
    wait_until("both deltas", || {
        seen.extend(deltas(&runtime));
        seen.len() >= 2
    });
    assert_eq!(seen, [1, 2]);
    assert!(dropped.load(Ordering::SeqCst), "turn 1 was not dropped");
}
