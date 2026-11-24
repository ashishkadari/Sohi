use std::cell::RefCell;
use std::collections::VecDeque;

use sohi_core::chat::{Role, ToolCall};

use super::*;
use crate::actions::tests::{Bench, call};

/// Answers round after round as scripted, keeping what it was sent.
struct Script {
    rounds: RefCell<VecDeque<(&'static str, Vec<ToolCall>)>>,
    sent: RefCell<Vec<Vec<Message>>>,
}

impl Script {
    fn new(rounds: impl IntoIterator<Item = (&'static str, Vec<ToolCall>)>) -> Self {
        Self {
            rounds: RefCell::new(rounds.into_iter().collect()),
            sent: RefCell::default(),
        }
    }
}

impl Model for Script {
    fn ask(
        &self,
        messages: &[Message],
        _provider: &mut Provider,
        on_text: &dyn Fn(&str),
        text: &mut String,
    ) -> impl Future<Output = Result<Completion, ProviderError>> {
        self.sent.borrow_mut().push(messages.to_vec());
        let (said, calls) = self.rounds.borrow_mut().pop_front().unwrap_or_default();
        if !said.is_empty() {
            on_text(said);
            text.push_str(said);
        }
        let blocks = None;
        std::future::ready(Ok(Completion { calls, blocks }))
    }
}

fn turn(bench: &Bench, prompt: &str) -> ChatTurn {
    ChatTurn {
        turn: 1,
        model: String::new(),
        fallback_model: String::new(),
        offline: None,
        system: String::new(),
        messages: vec![Message::user(prompt)],
        policy: bench.policy.clone(),
    }
}

/// Runs a turn; returns the reply and what was shown of it.
async fn run(bench: &Bench, script: &Script) -> (String, String) {
    let shown = RefCell::new(String::new());
    let on_text = |delta: &str| shown.borrow_mut().push_str(delta);
    let mut actions = bench.actions(NoEar);
    let reply = converse(&turn(bench, "go"), script, &mut actions, &on_text)
        .await
        .unwrap();
    drop(actions);
    (reply, shown.into_inner())
}

#[tokio::test]
async fn a_tool_runs_and_its_result_goes_back() {
    let bench = Bench::new("Sohi.Test.Reply1", "");
    let volume = call("volume", r#"{"action":"set","level":30}"#);
    let script = Script::new([
        ("On it.", vec![volume.clone()]),
        ("Volume's at 30.", vec![]),
    ]);
    let (reply, shown) = run(&bench, &script).await;
    assert_eq!(reply, "On it. Volume's at 30.");
    assert_eq!(shown, reply);
    assert_eq!(bench.ran(), ["volume"]);

    let sent = script.sent.borrow();
    assert_eq!(sent.len(), 2);
    let [user, calls, result] = &sent[1][..] else {
        panic!("{sent:?}");
    };
    assert_eq!(user, &Message::user("go"));
    assert_eq!(calls.content, "On it.");
    assert_eq!(calls.calls, [volume]);
    assert_eq!((result.role, result.call_id.as_str()), (Role::Tool, "id"));
    assert_eq!(result.content, r#"{"ok":true,"result":"Done."}"#);
}

#[tokio::test]
async fn several_calls_run_in_order() {
    let bench = Bench::new("Sohi.Test.Reply2", "");
    let calls = vec![
        call("launch_app", r#"{"name":"notepad"}"#),
        call("nonsense", "{}"),
        call("media", r#"{"action":"next"}"#),
    ];
    let script = Script::new([("", calls), ("", vec![])]);
    let (reply, shown) = run(&bench, &script).await;
    assert_eq!(bench.ran(), ["launch_app", "media"]);
    // The model said nothing: what happened is said for it.
    assert_eq!(reply, "Done.");
    assert_eq!(shown, reply);
    assert_eq!(script.sent.borrow()[1].len(), 5);
}

#[tokio::test]
async fn a_model_that_never_stops_calling_is_stopped() {
    let bench = Bench::new("Sohi.Test.Reply3", "");
    let again = || ("", vec![call("volume", r#"{"action":"up"}"#)]);
    let script = Script::new((0..50).map(|_| again()));
    run(&bench, &script).await;
    assert_eq!(bench.ran().len(), MAX_ROUNDS);
}

/// A call in the same round as the clipboard's, and the calls after it,
/// are asked about too.
#[tokio::test]
async fn calls_next_to_a_clipboard_read_are_asked_about() {
    let bench = Bench::new("Sohi.Test.Reply5", "x");
    let calls = vec![
        call("clipboard_get", ""),
        call("open_url", r#"{"url":"https://a.com"}"#),
    ];
    let script = Script::new([("", calls), ("Ok.", vec![])]);
    let user = async {
        bench.answer_next(true).await;
        bench.answer_next(true).await
    };
    let (_, asked) = tokio::join!(run(&bench, &script), user);
    assert_eq!(asked, "open https://a.com");
    assert_eq!(bench.ran(), ["clipboard_get", "open_url"]);
}
