//! Turns on the runtime thread (spec §5.5).
//!
//! The UI sends a [`Request::Chat`] with the whole conversation, or a
//! [`Request::Listen`] for a voice turn (see [`crate::voice`]); the
//! runtime streams the reply back as [`Update`]s. Prompts and replies are
//! never logged at `info` (spec §9.5); only timings and outcomes are.

use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Instant;

use sohi_core::chat::{Message, ToolRun};
use sohi_core::config::{SttConfig, TtsConfig};
use sohi_providers::{HttpClient, ProviderError};
use sohi_tools::Policy;
use tokio::sync::mpsc::Receiver;

use crate::actions::Toolbox;
use crate::runtime::UiLink;

/// UI → runtime.
#[derive(Debug)]
pub enum Request {
    /// Start a text turn. A turn still running is dropped first: exactly
    /// one turn runs at a time (spec §9.3).
    Chat(ChatTurn),
    /// Start a voice turn: listen, transcribe, then answer like a text
    /// turn. Replaces a running turn the same way.
    Listen(Box<ListenTurn>),
    /// Find out whether the online services answer. Runs beside any turn.
    Probe {
        /// Echoed in [`Update::Probed`].
        id: u64,
    },
    /// The user answered a tool question.
    Answer {
        /// The turn that asked.
        turn: u64,
        /// Which of its questions.
        ask: u32,
        /// The answer.
        yes: bool,
    },
    /// Cancel `turn` if it is the one running. `at` is when the user asked,
    /// for the latency log.
    Cancel {
        /// The turn to cancel.
        turn: u64,
        /// When the user asked.
        at: Instant,
    },
}

/// One text turn.
#[derive(Debug)]
pub struct ChatTurn {
    /// The interaction's turn id, echoed in every update.
    pub turn: u64,
    /// The Mistral model.
    pub model: String,
    /// The Anthropic model to fall back to; empty for no fallback.
    pub fallback_model: String,
    /// Answer without the internet, with this Ollama model (empty for
    /// none).
    pub offline: Option<String>,
    /// The system prompt.
    pub system: String,
    /// The conversation, ending with the new prompt.
    pub messages: Vec<Message>,
    /// Which tools may be used, and how.
    pub policy: Policy,
}

/// One voice turn: its [`ChatTurn`] holds the conversation *before* the
/// transcript, which the runtime appends.
#[derive(Debug)]
pub struct ListenTurn {
    /// The chat that follows the transcript.
    pub chat: ChatTurn,
    /// `[stt]` settings.
    pub stt: SttConfig,
    /// `[tts]` settings.
    pub tts: TtsConfig,
}

/// Runtime → UI.
#[derive(Debug)]
pub enum Update {
    /// The level of the mic, or of Sohi's own speech (RMS, 0 to 1), about
    /// 30 to 60 times a second.
    Level {
        /// The turn.
        turn: u64,
        /// The level.
        level: f32,
    },
    /// The user started speaking.
    SpeechStarted {
        /// The turn.
        turn: u64,
    },
    /// The utterance ended; the mic is closed and it is being transcribed.
    SpeechEnded {
        /// The turn.
        turn: u64,
    },
    /// Nobody spoke; the mic is closed.
    NoSpeech {
        /// The turn.
        turn: u64,
    },
    /// What was said. Empty if nothing was understood; otherwise the
    /// reply follows as for a text turn.
    Transcript {
        /// The turn.
        turn: u64,
        /// The transcript.
        text: String,
    },
    /// The mic could not be opened or stopped delivering audio.
    MicFailed {
        /// The turn.
        turn: u64,
    },
    /// The next piece of the reply.
    Delta {
        /// The turn.
        turn: u64,
        /// The text.
        text: String,
    },
    /// The reply is complete.
    Done {
        /// The turn.
        turn: u64,
        /// The whole reply, for the history.
        reply: String,
        /// The tools it took, for the history.
        runs: Vec<ToolRun>,
    },
    /// A tool call needs the user's permission.
    Question {
        /// The turn.
        turn: u64,
        /// Which of its questions this is.
        ask: u32,
        /// What would run.
        summary: String,
    },
    /// A question was answered by voice, or got no answer in time.
    Answered {
        /// The turn.
        turn: u64,
        /// The question.
        ask: u32,
        /// The answer.
        yes: bool,
    },
    /// The reply to a voice turn is being heard; its level follows as
    /// [`Update::Level`].
    SpeakingStarted {
        /// The turn.
        turn: u64,
    },
    /// A voice turn's reply was heard to the end. Comes after `Done`.
    PlaybackDone {
        /// The turn.
        turn: u64,
    },
    /// A probe ended.
    Probed {
        /// The probe.
        id: u64,
        /// Whether the service answered.
        reached: bool,
    },
    /// The turn failed.
    Failed {
        /// The turn.
        turn: u64,
        /// Why.
        error: ProviderError,
    },
}

/// A turn to run.
#[derive(Debug)]
enum Job {
    Chat(ChatTurn),
    Listen(Box<ListenTurn>),
}

impl Job {
    fn id(&self) -> u64 {
        match self {
            Self::Chat(turn) => turn.turn,
            Self::Listen(listen) => listen.chat.turn,
        }
    }
}

/// Runs one turn to completion, sending its updates. Dropping the future
/// cancels the turn, including any HTTP request in flight and the mic.
trait TurnRunner {
    async fn run(&mut self, job: Job, ui: &UiLink<Update>);
}

/// Turns against the online providers; the HTTP client is created on the
/// first turn and shared after that, so connections are pooled.
#[derive(Default)]
pub struct ProviderRunner {
    http: SharedClient,
    /// What tools run on.
    pub toolbox: Toolbox,
    /// When the connection was last warmed (spec §3.3: cached 60 s).
    pub prewarmed: Option<Instant>,
}

/// The HTTP client of the runtime thread, once something needed it.
pub type SharedClient = Rc<OnceCell<HttpClient>>;

/// The shared client, created on first use.
///
/// # Errors
/// Returns [`ProviderError::Offline`] if the TLS backend cannot start.
pub fn shared_client(shared: &SharedClient) -> Result<HttpClient, ProviderError> {
    if let Some(http) = shared.get() {
        return Ok(http.clone());
    }
    let http = HttpClient::new()?;
    Ok(shared.get_or_init(|| http).clone())
}

impl ProviderRunner {
    /// The shared HTTP client, created on first use.
    ///
    /// # Errors
    /// Returns [`ProviderError::Offline`] if the TLS backend cannot start.
    pub fn client(&mut self) -> Result<HttpClient, ProviderError> {
        shared_client(&self.http)
    }
}

impl TurnRunner for ProviderRunner {
    async fn run(&mut self, job: Job, ui: &UiLink<Update>) {
        match job {
            Job::Chat(turn) => {
                let id = turn.turn;
                match self.client() {
                    Ok(http) => match &turn.offline {
                        Some(model) => {
                            crate::offline::answer(&http, &turn, model, &self.toolbox, ui).await;
                        }
                        None => crate::reply::answer(&http, &turn, &self.toolbox, ui).await,
                    },
                    Err(error) => {
                        log::warn!("turn {id} failed: {error}");
                        ui.send(Update::Failed { turn: id, error });
                    }
                }
            }
            Job::Listen(listen) => crate::voice::run(self, *listen, ui).await,
        }
    }
}

/// Serves requests until the UI drops its handle.
pub async fn serve(requests: Receiver<Request>, ui: UiLink<Update>) {
    let runner = ProviderRunner::default();
    let http = Rc::clone(&runner.http);
    let answers = runner.toolbox.answers();
    let aside = move |aside, ui: &UiLink<Update>| match aside {
        Aside::Probe { id } => crate::probe::start(&http, id, ui),
        Aside::Answer { turn, ask, yes } => answers.give(turn, ask, yes),
    };
    serve_with(requests, ui, runner, aside).await;
}

/// What is done beside any turn.
enum Aside {
    Probe { id: u64 },
    Answer { turn: u64, ask: u32, yes: bool },
}

async fn serve_with(
    mut requests: Receiver<Request>,
    ui: UiLink<Update>,
    mut runner: impl TurnRunner,
    aside: impl Fn(Aside, &UiLink<Update>),
) {
    let mut next = None;
    loop {
        let request = match next.take() {
            Some(request) => request,
            None => match requests.recv().await {
                Some(request) => request,
                None => return,
            },
        };
        // A cancel with nothing running has nothing to do.
        let job = match request {
            Request::Chat(turn) => Job::Chat(turn),
            Request::Listen(listen) => Job::Listen(listen),
            Request::Cancel { .. } => continue,
            Request::Answer { turn, ask, yes } => {
                aside(Aside::Answer { turn, ask, yes }, &ui);
                continue;
            }
            Request::Probe { id } => {
                aside(Aside::Probe { id }, &ui);
                continue;
            }
        };
        match run_cancellable(&mut runner, job, &mut requests, &ui, &aside).await {
            Outcome::Finished => {}
            Outcome::Cancelled { turn, at } => log::info!(
                "turn {turn} cancelled {:.1} ms after the request; its HTTP request was dropped",
                at.elapsed().as_secs_f64() * 1000.0
            ),
            Outcome::Replaced(request) => next = Some(request),
            Outcome::Closed => return,
        }
    }
}

/// How a running turn ended.
enum Outcome {
    Finished,
    Cancelled {
        turn: u64,
        at: Instant,
    },
    /// A new turn arrived; this one was dropped.
    Replaced(Request),
    /// The UI went away.
    Closed,
}

/// Runs `turn` while watching for requests. The turn's future is dropped
/// before this returns, so a cancelled turn's HTTP request is gone by the
/// time the caller logs it.
async fn run_cancellable(
    runner: &mut impl TurnRunner,
    job: Job,
    requests: &mut Receiver<Request>,
    ui: &UiLink<Update>,
    aside: &impl Fn(Aside, &UiLink<Update>),
) -> Outcome {
    let id = job.id();
    let run = runner.run(job, ui);
    tokio::pin!(run);
    loop {
        tokio::select! {
            // The turn first: it gets polled at least once (so it starts)
            // and a turn that is already done finishes rather than being
            // reported as cancelled. Random order made this flaky.
            biased;
            () = &mut run => return Outcome::Finished,
            request = requests.recv() => match request {
                Some(Request::Cancel { turn, at }) if turn == id => {
                    return Outcome::Cancelled { turn, at };
                }
                // Cancelling a turn that already ended: keep going.
                Some(Request::Cancel { .. }) => {}
                Some(Request::Answer { turn, ask, yes }) => {
                    aside(Aside::Answer { turn, ask, yes }, ui);
                }
                Some(Request::Probe { id }) => aside(Aside::Probe { id }, ui),
                Some(request) => {
                    log::info!("turn {id} replaced by a newer one");
                    return Outcome::Replaced(request);
                }
                None => return Outcome::Closed,
            },
        }
    }
}

#[cfg(test)]
mod tests;
