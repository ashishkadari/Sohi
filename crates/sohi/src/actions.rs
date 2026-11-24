//! Running the tools a turn calls: judging each call, asking the user
//! about it if need be, and doing it off the runtime thread (spec §5.6).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sohi_core::chat::ToolCall;
use sohi_tools::{
    Call, Desktop, Executor, Gate, Outcome, Permit, Policy, Question, Verdict, WinDesktop,
};
use tokio::sync::oneshot;

use crate::chat::Update;
use crate::runtime::UiLink;

/// A question nobody answers for this long is a no (spec §5.6).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Whatever runs permitted calls.
trait Run: Send {
    fn run(&mut self, permit: &Permit) -> Outcome;
}

impl<D: Desktop> Run for Executor<D> {
    fn run(&mut self, permit: &Permit) -> Outcome {
        Executor::run(self, permit)
    }
}

/// What the turns of this run share: the executor with its app index, and
/// the question being asked.
#[derive(Clone)]
pub struct Toolbox {
    executor: Arc<Mutex<dyn Run>>,
    answers: Answers,
}

impl Default for Toolbox {
    fn default() -> Self {
        Self::on(Executor::new(WinDesktop))
    }
}

impl Toolbox {
    fn on(executor: impl Run + 'static) -> Self {
        Self {
            executor: Arc::new(Mutex::new(executor)),
            answers: Answers::default(),
        }
    }

    /// Where the user's answers go.
    pub fn answers(&self) -> Answers {
        self.answers.clone()
    }

    /// Runs what was permitted, on a thread that may block.
    async fn run(&self, permit: Permit) -> Outcome {
        let executor = Arc::clone(&self.executor);
        let run = move || {
            executor
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .run(&permit)
        };
        tokio::task::spawn_blocking(run)
            .await
            .unwrap_or_else(|err| {
                log::error!("a tool died: {err}");
                Outcome::failed("the tool failed")
            })
    }
}

/// Carries the user's answer to the question waiting for it.
#[derive(Clone, Default)]
pub struct Answers(Rc<RefCell<Option<Waiting>>>);

struct Waiting {
    turn: u64,
    ask: u32,
    answer: oneshot::Sender<bool>,
}

impl Answers {
    /// Takes an answer. One to a question that is not waiting any more is
    /// dropped.
    pub fn give(&self, turn: u64, ask: u32, yes: bool) {
        let mut waiting = self.0.borrow_mut();
        match waiting.take() {
            Some(w) if w.turn == turn && w.ask == ask => {
                let _ = w.answer.send(yes);
            }
            other => {
                log::info!("turn {turn}: late answer to question {ask} dropped");
                *waiting = other;
            }
        }
    }

    fn wait_for(&self, turn: u64, ask: u32) -> oneshot::Receiver<bool> {
        let (answer, receiver) = oneshot::channel();
        *self.0.borrow_mut() = Some(Waiting { turn, ask, answer });
        receiver
    }
}

/// How a spoken answer is heard.
pub trait Ear {
    /// Waits until the question can be asked (Sohi is not talking).
    async fn ready(&self);

    /// Listens for a spoken yes or no; `None` if there was none.
    async fn listen(&self) -> Option<bool>;
}

/// A typed turn: nobody listens.
pub struct NoEar;

impl Ear for NoEar {
    async fn ready(&self) {}

    fn listen(&self) -> impl Future<Output = Option<bool>> {
        std::future::ready(None)
    }
}

/// The tool calls of one turn.
pub struct Actions<'a, E> {
    turn: u64,
    gate: Gate<'a>,
    toolbox: &'a Toolbox,
    ui: &'a UiLink<Update>,
    ear: E,
    asks: u32,
    last: String,
    timeout: Duration,
}

impl<'a, E: Ear> Actions<'a, E> {
    /// Nothing called yet.
    pub fn new(
        turn: u64,
        policy: &'a Policy,
        toolbox: &'a Toolbox,
        ui: &'a UiLink<Update>,
        ear: E,
    ) -> Self {
        Self {
            turn,
            gate: policy.gate(),
            toolbox,
            ui,
            ear,
            asks: 0,
            last: String::new(),
            timeout: ANSWER_TIMEOUT,
        }
    }

    /// What the latest call did, for the user.
    pub fn last_message(&self) -> &str {
        &self.last
    }

    /// Runs a model's call and returns the result for it. `shown` is the
    /// reply so far.
    pub async fn run(&mut self, call: &ToolCall, shown: &str) -> String {
        let outcome = match Call::parse(&call.name, &call.arguments) {
            Ok(call) => {
                // Spec §5.6: the address is shown in the bubble.
                let opens_url = matches!(call, Call::OpenUrl { .. });
                let outcome = self.execute(call).await;
                if opens_url && outcome.ok {
                    let before = if shown.is_empty() { "" } else { "\n" };
                    self.ui.send(Update::Delta {
                        turn: self.turn,
                        text: format!("{before}→ {}\n", outcome.message),
                    });
                }
                outcome
            }
            Err(error) => {
                log::warn!("turn {}: unusable tool call: {error}", self.turn);
                Outcome::failed(error.to_string())
            }
        };
        self.last.clone_from(&outcome.message);
        outcome.to_json()
    }

    /// Judges and runs `call`.
    pub async fn execute(&mut self, call: Call) -> Outcome {
        let name = call.tool().name();
        let started = Instant::now();
        let outcome = match self.gate.judge(call) {
            Verdict::Refuse(outcome) => outcome,
            Verdict::Allow(permit) => self.toolbox.run(permit).await,
            Verdict::Ask(question) => match self.ask(question).await {
                Ok(permit) => self.toolbox.run(permit).await,
                Err(outcome) => outcome,
            },
        };
        self.gate.saw(&outcome);
        // Never the arguments or what came back (spec §9.5).
        log::info!(
            "turn {}: {name} {} in {} ms",
            self.turn,
            if outcome.ok { "ran" } else { "did not run" },
            started.elapsed().as_millis()
        );
        outcome
    }

    /// Asks the user. No answer is a no.
    async fn ask(&mut self, question: Question) -> Result<Permit, Outcome> {
        self.ear.ready().await;
        self.asks += 1;
        let (turn, ask) = (self.turn, self.asks);
        let answer = self.toolbox.answers.wait_for(turn, ask);
        log::info!("turn {turn}: question {ask} asked");
        self.ui.send(Update::Question {
            turn,
            ask,
            summary: question.summary(),
        });
        let spoken = async {
            match self.ear.listen().await {
                Some(yes) => yes,
                // Silence, or something else: the keys still count.
                None => std::future::pending().await,
            }
        };
        let elsewhere = async {
            let yes = tokio::time::timeout(self.timeout, spoken).await;
            yes.unwrap_or(false)
        };
        let yes = tokio::select! {
            biased;
            yes = answer => yes.unwrap_or(false),
            yes = elsewhere => {
                log::info!("turn {turn}: question {ask} ended {yes}");
                self.ui.send(Update::Answered { turn, ask, yes });
                yes
            }
        };
        self.toolbox.answers.0.borrow_mut().take();
        question.answer(yes)
    }
}

#[cfg(test)]
pub(crate) mod tests;
