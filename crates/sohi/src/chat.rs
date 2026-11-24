//! Turns on the runtime thread (spec §5.5).
//!
//! The UI sends a [`Request::Chat`] with the whole conversation, or a
//! [`Request::Listen`] for a voice turn (see [`crate::voice`]); the
//! runtime streams the reply back as [`Update`]s. Prompts and replies are
//! never logged at `info` (spec §9.5); only timings and outcomes are.

use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Instant;

use sohi_core::chat::Message;
use sohi_core::config::{SttConfig, TtsConfig};
use sohi_core::secret::{KeyName, Secret};
use sohi_providers::retry::{jitter_seed, retry_delay, retry_once};
use sohi_providers::{
    Anthropic, ChatRequest, HttpClient, Mistral, ProviderError, anthropic, mistral,
};
use tokio::sync::mpsc::Receiver;

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
                        Some(model) => crate::offline::answer(&http, &turn, model, ui).await,
                        None => answer(&http, &turn, ui).await,
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
    let probe = move |id, ui: &UiLink<Update>| crate::probe::start(&http, id, ui);
    serve_with(requests, ui, runner, probe).await;
}

async fn serve_with(
    mut requests: Receiver<Request>,
    ui: UiLink<Update>,
    mut runner: impl TurnRunner,
    probe: impl Fn(u64, &UiLink<Update>),
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
            Request::Probe { id } => {
                probe(id, &ui);
                continue;
            }
        };
        match run_cancellable(&mut runner, job, &mut requests, &ui, &probe).await {
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
    probe: &impl Fn(u64, &UiLink<Update>),
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
                Some(Request::Probe { id }) => probe(id, ui),
                Some(request) => {
                    log::info!("turn {id} replaced by a newer one");
                    return Outcome::Replaced(request);
                }
                None => return Outcome::Closed,
            },
        }
    }
}

/// Streams the reply to `turn` and reports how it ended.
pub async fn answer(http: &HttpClient, turn: &ChatTurn, ui: &UiLink<Update>) {
    let id = turn.turn;
    let on_text = |delta: &str| {
        ui.send(Update::Delta {
            turn: id,
            text: delta.to_owned(),
        });
    };
    match chat(http, turn, &on_text).await {
        Ok(reply) => {
            ui.send(Update::Done { turn: id, reply });
        }
        Err(error) => {
            log::warn!("turn {id} failed: {error}");
            ui.send(Update::Failed { turn: id, error });
        }
    }
}

/// Asks Mistral, then the fallback provider if Mistral failed before
/// writing anything (spec §2.3). A reply already under way is never
/// restarted elsewhere: the user would see it twice.
/// Calls `on_text` with each piece; returns the whole reply.
pub async fn chat(
    http: &HttpClient,
    turn: &ChatTurn,
    on_text: &dyn Fn(&str),
) -> Result<String, ProviderError> {
    let mut reply = String::new();
    let primary = attempt(
        Provider::Mistral,
        http,
        &turn.model,
        turn,
        on_text,
        &mut reply,
    )
    .await;
    let error = match primary {
        Ok(()) => return Ok(reply),
        Err(error) => error,
    };
    if !reply.is_empty() || turn.fallback_model.is_empty() || error == ProviderError::Cancelled {
        return Err(error);
    }
    log::info!(
        "turn {}: Mistral failed ({error}); trying Anthropic",
        turn.turn
    );
    match attempt(
        Provider::Anthropic,
        http,
        &turn.fallback_model,
        turn,
        on_text,
        &mut reply,
    )
    .await
    {
        Ok(()) => Ok(reply),
        // A fallback that was never set up says nothing new: report why
        // the primary failed.
        Err(ProviderError::MissingKey(_)) => Err(error),
        Err(fallback) => Err(fallback),
    }
}

#[derive(Debug, Clone, Copy)]
enum Provider {
    Mistral,
    Anthropic,
}

impl Provider {
    fn key(self) -> KeyName {
        match self {
            Self::Mistral => KeyName::Mistral,
            Self::Anthropic => KeyName::Anthropic,
        }
    }
}

/// One provider, with one retry for a 429 or 5xx before any text.
async fn attempt(
    provider: Provider,
    http: &HttpClient,
    model: &str,
    turn: &ChatTurn,
    on_text: &dyn Fn(&str),
    reply: &mut String,
) -> Result<(), ProviderError> {
    let delay = retry_delay(jitter_seed());
    let on_retry = |error: &ProviderError| {
        log::info!(
            "turn {}: {provider:?} failed ({error}); retrying in {} ms",
            turn.turn,
            delay.as_millis()
        );
    };
    retry_once(reply, delay, on_retry, async |reply: &mut String| {
        stream(provider, http, model, turn, on_text, reply).await
    })
    .await
}

/// Streams one attempt into `reply`, forwarding each piece.
async fn stream(
    provider: Provider,
    http: &HttpClient,
    model: &str,
    turn: &ChatTurn,
    on_text: &dyn Fn(&str),
    reply: &mut String,
) -> Result<(), ProviderError> {
    let key = read_key(provider.key())?;
    let request = ChatRequest {
        model,
        system: &turn.system,
        messages: &turn.messages,
    };
    let started = Instant::now();
    log::info!(
        "turn {} started on {provider:?} ({} messages)",
        turn.turn,
        turn.messages.len()
    );
    let mut on_delta = |delta: &str| {
        if reply.is_empty() {
            log::info!(
                "turn {} first token after {} ms",
                turn.turn,
                started.elapsed().as_millis()
            );
        }
        reply.push_str(delta);
        on_text(delta);
    };
    match provider {
        Provider::Mistral => {
            Mistral::new(http.clone(), mistral::BASE_URL)
                .chat_stream(&key, &request, &mut on_delta)
                .await?;
        }
        Provider::Anthropic => {
            Anthropic::new(http.clone(), anthropic::BASE_URL)
                .chat_stream(&key, &request, &mut on_delta)
                .await?;
        }
    }
    log::info!(
        "turn {} done in {} ms ({} chars)",
        turn.turn,
        started.elapsed().as_millis(),
        reply.chars().count()
    );
    Ok(())
}

pub(crate) fn read_key(name: KeyName) -> Result<Secret, ProviderError> {
    match sohi_win::read_key(name.target()) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(ProviderError::MissingKey(name.as_str())),
        Err(err) => {
            log::warn!("cannot read the {} key: {err}", name.as_str());
            Err(ProviderError::MissingKey(name.as_str()))
        }
    }
}

#[cfg(test)]
mod tests;
