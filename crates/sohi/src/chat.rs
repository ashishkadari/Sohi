//! Text turns on the runtime thread (spec §5.5, text path).
//!
//! The UI sends a [`Request::Chat`] with the whole conversation; the
//! runtime streams the reply back as [`Update`]s. Prompts and replies are
//! never logged at `info` (spec §9.5); only timings and outcomes are.

use std::time::Instant;

use sohi_core::chat::Message;
use sohi_core::secret::KeyName;
use sohi_providers::{ChatRequest, HttpClient, Mistral, ProviderError, mistral};
use tokio::sync::mpsc::Receiver;

use crate::runtime::UiLink;

/// UI → runtime.
#[derive(Debug)]
pub enum Request {
    /// Start a text turn.
    Chat(ChatTurn),
}

/// One text turn.
#[derive(Debug)]
pub struct ChatTurn {
    /// The interaction's turn id, echoed in every update.
    pub turn: u64,
    /// The model name.
    pub model: String,
    /// The system prompt.
    pub system: String,
    /// The conversation, ending with the new prompt.
    pub messages: Vec<Message>,
}

/// Runtime → UI.
#[derive(Debug)]
pub enum Update {
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
    /// The turn failed.
    Failed {
        /// The turn.
        turn: u64,
        /// Why.
        error: ProviderError,
    },
}

/// Serves requests until the UI drops its handle.
pub async fn serve(mut requests: Receiver<Request>, ui: UiLink<Update>) {
    let mut client = None;
    while let Some(request) = requests.recv().await {
        match request {
            Request::Chat(turn) => run_turn(&mut client, turn, &ui).await,
        }
    }
}

async fn run_turn(client: &mut Option<Mistral>, turn: ChatTurn, ui: &UiLink<Update>) {
    let id = turn.turn;
    match chat(client, &turn, ui).await {
        Ok(reply) => {
            ui.send(Update::Done { turn: id, reply });
        }
        Err(error) => {
            log::warn!("turn {id} failed: {error}");
            ui.send(Update::Failed { turn: id, error });
        }
    }
}

async fn chat(
    client: &mut Option<Mistral>,
    turn: &ChatTurn,
    ui: &UiLink<Update>,
) -> Result<String, ProviderError> {
    let key = match sohi_win::read_key(KeyName::Mistral.target()) {
        Ok(Some(key)) => key,
        Ok(None) => return Err(ProviderError::MissingKey(KeyName::Mistral.as_str())),
        Err(err) => {
            log::warn!("cannot read the Mistral key: {err}");
            return Err(ProviderError::MissingKey(KeyName::Mistral.as_str()));
        }
    };
    let client = match client {
        Some(client) => client,
        None => client.insert(Mistral::new(HttpClient::new()?, mistral::BASE_URL)),
    };
    let request = ChatRequest {
        model: &turn.model,
        system: &turn.system,
        messages: &turn.messages,
    };
    let started = Instant::now();
    let mut reply = String::new();
    log::info!(
        "turn {} started ({} messages)",
        turn.turn,
        turn.messages.len()
    );
    client
        .chat_stream(&key, &request, &mut |delta| {
            if reply.is_empty() {
                log::info!(
                    "turn {} first token after {} ms",
                    turn.turn,
                    started.elapsed().as_millis()
                );
            }
            reply.push_str(delta);
            ui.send(Update::Delta {
                turn: turn.turn,
                text: delta.to_owned(),
            });
        })
        .await?;
    log::info!(
        "turn {} done in {} ms ({} chars)",
        turn.turn,
        started.elapsed().as_millis(),
        reply.chars().count()
    );
    Ok(reply)
}
