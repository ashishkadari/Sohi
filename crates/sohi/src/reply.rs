//! Getting a reply from the online models, running the tools they call on
//! the way (spec §2.3, §5.6).

use std::time::Instant;

use sohi_core::chat::{Message, ToolSpec, without_calls};
use sohi_core::secret::{KeyName, Secret};
use sohi_providers::retry::{jitter_seed, retry_delay, retry_once};
use sohi_providers::{
    Anthropic, ChatRequest, Completion, HttpClient, Mistral, ProviderError, anthropic, mistral,
};

use crate::actions::{Actions, Ear, NoEar, Toolbox};
use crate::chat::{ChatTurn, Update};
use crate::runtime::UiLink;

/// A model that has called tools this many times over is stopped.
const MAX_ROUNDS: usize = 5;

/// Streams the reply to `turn` and reports how it ended.
pub async fn answer(http: &HttpClient, turn: &ChatTurn, toolbox: &Toolbox, ui: &UiLink<Update>) {
    let id = turn.turn;
    let on_text = |delta: &str| {
        ui.send(Update::Delta {
            turn: id,
            text: delta.to_owned(),
        });
    };
    let mut actions = Actions::new(id, &turn.policy, toolbox, ui, NoEar);
    match chat(http, turn, &mut actions, &on_text).await {
        Ok(reply) => {
            let runs = actions.take_runs();
            ui.send(Update::Done {
                turn: id,
                reply,
                runs,
            });
        }
        Err(error) => {
            log::warn!("turn {id} failed: {error}");
            ui.send(Update::Failed { turn: id, error });
        }
    }
}

/// Asks the models, running the tools they call, until they have no more
/// to call. Calls `on_text` with each piece; returns the whole reply.
pub async fn chat<E: Ear>(
    http: &HttpClient,
    turn: &ChatTurn,
    actions: &mut Actions<'_, E>,
    on_text: &dyn Fn(&str),
) -> Result<String, ProviderError> {
    let specs = turn.policy.specs();
    let online = Online {
        http,
        turn,
        specs: &specs,
    };
    converse(turn, &online, actions, on_text).await
}

/// Where a round's reply comes from.
trait Model {
    /// Asks once. `text` collects what is written, `on_text` is called
    /// with each piece.
    async fn ask(
        &self,
        messages: &[Message],
        provider: &mut Provider,
        on_text: &dyn Fn(&str),
        text: &mut String,
    ) -> Result<Completion, ProviderError>;
}

async fn converse<E: Ear>(
    turn: &ChatTurn,
    model: &impl Model,
    actions: &mut Actions<'_, E>,
    on_text: &dyn Fn(&str),
) -> Result<String, ProviderError> {
    let mut messages = turn.messages.clone();
    let mut provider = Provider::Mistral;
    let mut reply = String::new();
    let mut rounds = 0;
    loop {
        let gap = needs_gap(&reply);
        let first = std::cell::Cell::new(gap);
        let spaced = |delta: &str| {
            if first.take() {
                on_text(" ");
            }
            on_text(delta);
        };
        let mut text = String::new();
        let done = model
            .ask(&messages, &mut provider, &spaced, &mut text)
            .await?;
        if gap && !text.is_empty() {
            reply.push(' ');
        }
        reply.push_str(&text);
        if done.calls.is_empty() {
            break;
        }
        if rounds == MAX_ROUNDS {
            log::warn!("turn {}: too many tool rounds", turn.turn);
            break;
        }
        rounds += 1;
        messages.push(Message::calls(text, done.calls.clone(), done.blocks));
        for call in &done.calls {
            let result = actions.run(call, &reply).await;
            messages.push(Message::tool_result(call.id.clone(), result));
        }
    }
    // Some models say nothing once their tool has run.
    if reply.trim().is_empty() && rounds > 0 {
        let said = actions.last_message();
        on_text(said);
        reply.push_str(said);
    }
    Ok(reply)
}

/// Whether more text must be set apart from `reply` by a space.
fn needs_gap(reply: &str) -> bool {
    reply.chars().last().is_some_and(|c| !c.is_whitespace())
}

/// Mistral and Anthropic.
struct Online<'a> {
    http: &'a HttpClient,
    turn: &'a ChatTurn,
    specs: &'a [ToolSpec],
}

impl Model for Online<'_> {
    /// Asks Mistral, then the fallback provider if Mistral failed before
    /// writing anything (spec §2.3). A reply already under way is never
    /// restarted elsewhere: the user would see it twice. A turn that fell
    /// back stays there.
    async fn ask(
        &self,
        messages: &[Message],
        provider: &mut Provider,
        on_text: &dyn Fn(&str),
        text: &mut String,
    ) -> Result<Completion, ProviderError> {
        let turn = self.turn;
        let error = match self.attempt(*provider, messages, on_text, text).await {
            Ok(done) => return Ok(done),
            Err(error) => error,
        };
        if *provider == Provider::Anthropic
            || !text.is_empty()
            || turn.fallback_model.is_empty()
            || error == ProviderError::Cancelled
        {
            return Err(error);
        }
        log::info!(
            "turn {}: Mistral failed ({error}); trying Anthropic",
            turn.turn
        );
        *provider = Provider::Anthropic;
        match self.attempt(*provider, messages, on_text, text).await {
            // A fallback that was never set up says nothing new: report why
            // the primary failed.
            Err(ProviderError::MissingKey(_)) => Err(error),
            result => result,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl Online<'_> {
    /// One provider, with one retry for a 429 or 5xx before any text.
    async fn attempt(
        &self,
        provider: Provider,
        messages: &[Message],
        on_text: &dyn Fn(&str),
        text: &mut String,
    ) -> Result<Completion, ProviderError> {
        let delay = retry_delay(jitter_seed());
        let on_retry = |error: &ProviderError| {
            log::info!(
                "turn {}: {provider:?} failed ({error}); retrying in {} ms",
                self.turn.turn,
                delay.as_millis()
            );
        };
        let mut done = Completion::default();
        retry_once(text, delay, on_retry, async |text: &mut String| {
            done = self.stream(provider, messages, on_text, text).await?;
            Ok(())
        })
        .await?;
        Ok(done)
    }

    /// Streams one attempt into `text`, forwarding each piece.
    async fn stream(
        &self,
        provider: Provider,
        messages: &[Message],
        on_text: &dyn Fn(&str),
        text: &mut String,
    ) -> Result<Completion, ProviderError> {
        let id = self.turn.turn;
        let key = read_key(provider.key())?;
        let model = match provider {
            Provider::Mistral => &self.turn.model,
            Provider::Anthropic => &self.turn.fallback_model,
        };
        // Anthropic refuses calls of tools it is not told about.
        let plain;
        let messages = if self.specs.is_empty() {
            plain = without_calls(messages);
            &plain
        } else {
            messages
        };
        let request = ChatRequest {
            model,
            system: &self.turn.system,
            messages,
            tools: self.specs,
        };
        let started = Instant::now();
        log::info!(
            "turn {id} started on {provider:?} ({} messages)",
            messages.len()
        );
        let mut on_delta = |delta: &str| {
            if text.is_empty() {
                log::info!(
                    "turn {id} first token after {} ms",
                    started.elapsed().as_millis()
                );
            }
            text.push_str(delta);
            on_text(delta);
        };
        let done = match provider {
            Provider::Mistral => {
                Mistral::new(self.http.clone(), mistral::BASE_URL)
                    .chat_stream(&key, &request, &mut on_delta)
                    .await?
            }
            Provider::Anthropic => {
                Anthropic::new(self.http.clone(), anthropic::BASE_URL)
                    .chat_stream(&key, &request, &mut on_delta)
                    .await?
            }
        };
        log::info!(
            "turn {id} done in {} ms ({} chars, {} tool calls)",
            started.elapsed().as_millis(),
            text.chars().count(),
            done.calls.len()
        );
        Ok(done)
    }
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
