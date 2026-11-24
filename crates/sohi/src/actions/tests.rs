use std::sync::mpsc::Receiver;

use sohi_core::config::ToolsConfig;
use sohi_win::{AppWindow, EventQueue};

use super::*;

/// Notes what ran; reading the clipboard returns `clipboard`.
#[derive(Clone, Default)]
pub(crate) struct Notes {
    pub ran: Arc<Mutex<Vec<Call>>>,
    pub clipboard: String,
}

impl Run for Notes {
    fn run(&mut self, permit: &Permit) -> Outcome {
        self.ran.lock().unwrap().push(permit.call().clone());
        match permit.call() {
            Call::ClipboardGet => Outcome {
                untrusted: Some(self.clipboard.clone()),
                ..Outcome::done("Read the clipboard.")
            },
            Call::OpenUrl { url } => Outcome::done(format!("Opened {url}")),
            _ => Outcome::done("Done."),
        }
    }
}

/// Everything a turn's calls need.
pub(crate) struct Bench {
    pub notes: Notes,
    pub toolbox: Toolbox,
    pub policy: Policy,
    pub ui: UiLink<Update>,
    pub updates: Receiver<Update>,
    _window: AppWindow,
}

impl Bench {
    pub fn new(class: &str, clipboard: &str) -> Self {
        let window = AppWindow::create(class, 0xC150, EventQueue::new()).unwrap();
        let (ui, updates) = crate::runtime::test_link(window.remote());
        let notes = Notes {
            clipboard: clipboard.to_owned(),
            ..Notes::default()
        };
        Self {
            toolbox: Toolbox::on(notes.clone()),
            notes,
            policy: Policy::new(&ToolsConfig::default()),
            ui,
            updates,
            _window: window,
        }
    }

    pub fn actions<E: Ear>(&self, ear: E) -> Actions<'_, E> {
        Actions::new(1, &self.policy, &self.toolbox, &self.ui, ear)
    }

    pub fn ran(&self) -> Vec<&'static str> {
        let ran = self.notes.ran.lock().unwrap();
        ran.iter().map(|call| call.tool().name()).collect()
    }

    /// Answers the next question with `yes` as soon as it is asked;
    /// returns what was asked.
    pub async fn answer_next(&self, yes: bool) -> String {
        loop {
            for update in self.updates.try_iter() {
                if let Update::Question { turn, ask, summary } = update {
                    self.toolbox.answers().give(turn, ask, yes);
                    return summary;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
}

pub(crate) fn call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: "id".to_owned(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    }
}

const SOON: Duration = Duration::from_millis(40);

/// Hears this, at once.
struct Hears(Option<bool>);

impl Ear for Hears {
    async fn ready(&self) {}

    fn listen(&self) -> impl Future<Output = Option<bool>> {
        std::future::ready(self.0)
    }
}

#[tokio::test]
async fn allowed_calls_run_and_unusable_ones_do_not() {
    let bench = Bench::new("Sohi.Test.Actions1", "");
    let mut actions = bench.actions(NoEar);
    let result = actions
        .run(&call("open_url", r#"{"url":"a.com"}"#), "")
        .await;
    assert_eq!(result, r#"{"ok":true,"result":"Opened a.com"}"#);
    let result = actions.run(&call("format_disk", "{}"), "").await;
    assert!(result.contains("no such tool"), "{result}");
    let result = actions.run(&call("volume", "{"), "").await;
    assert!(result.contains("bad arguments"), "{result}");
    assert_eq!(bench.ran(), ["open_url"]);
    let shown: Vec<Update> = bench.updates.try_iter().collect();
    assert!(
        matches!(&shown[..], [Update::Delta { text, .. }] if text == "→ Opened a.com\n"),
        "{shown:?}"
    );
}

#[tokio::test]
async fn a_question_is_answered_by_the_keys() {
    for yes in [true, false] {
        let bench = Bench::new("Sohi.Test.Actions2", "text");
        let mut actions = bench.actions(NoEar);
        let get = call("clipboard_get", "");
        let (result, asked) = tokio::join!(actions.run(&get, ""), bench.answer_next(yes));
        assert_eq!(asked, "read your clipboard");
        assert_eq!(result.contains(r#""untrusted_data":"text""#), yes);
        assert_eq!(result.contains("denied"), !yes);
        assert_eq!(bench.ran().len(), usize::from(yes));
        // The UI knew already.
        assert!(
            !bench
                .updates
                .try_iter()
                .any(|u| matches!(u, Update::Answered { .. }))
        );
    }
}

#[tokio::test]
async fn a_question_is_answered_aloud() {
    for yes in [true, false] {
        let bench = Bench::new("Sohi.Test.Actions3", "text");
        let mut actions = bench.actions(Hears(Some(yes)));
        let result = actions.run(&call("clipboard_get", ""), "").await;
        assert_eq!(result.contains("denied"), !yes);
        let told = bench
            .updates
            .try_iter()
            .any(|u| matches!(u, Update::Answered { ask: 1, yes: y, .. } if y == yes));
        assert!(told);
    }
}

#[tokio::test]
async fn no_answer_in_time_is_a_no() {
    let bench = Bench::new("Sohi.Test.Actions4", "text");
    let mut actions = bench.actions(Hears(None));
    assert_eq!(actions.timeout, Duration::from_secs(10));
    actions.timeout = SOON;
    let began = Instant::now();
    let result = actions.run(&call("clipboard_get", ""), "").await;
    assert!(began.elapsed() >= SOON);
    assert!(result.contains("denied"));
    assert_eq!(bench.ran(), [""; 0]);
    assert!(
        bench
            .updates
            .try_iter()
            .any(|u| matches!(u, Update::Answered { yes: false, .. }))
    );
}

#[tokio::test]
async fn a_late_or_wrong_answer_changes_nothing() {
    let bench = Bench::new("Sohi.Test.Actions5", "text");
    let answers = bench.toolbox.answers();
    answers.give(1, 1, true);
    let mut actions = bench.actions(NoEar);
    actions.timeout = SOON;
    let get = call("clipboard_get", "");
    let wrong = async {
        tokio::time::sleep(Duration::from_millis(5)).await;
        answers.give(1, 2, true);
        answers.give(2, 1, true);
    };
    let (result, ()) = tokio::join!(actions.run(&get, ""), wrong);
    assert!(result.contains("denied"));
    assert_eq!(bench.ran(), [""; 0]);
}
