//! Typed turns while offline (spec §5.3): a local Ollama model if one is
//! configured and running, otherwise the command grammar.

use std::time::Instant;

use sohi_core::intent::{self, Intent};
use sohi_core::persona::LocalTime;
use sohi_providers::{ChatRequest, HttpClient, Ollama, ProviderError, ollama};

use crate::chat::{ChatTurn, Update};
use crate::runtime::UiLink;

/// Answers `turn` without the internet, using `model` if possible.
pub async fn answer(http: &HttpClient, turn: &ChatTurn, model: &str, ui: &UiLink<Update>) {
    let id = turn.turn;
    let mut reply = String::new();
    let mut on_delta = |delta: &str| {
        reply.push_str(delta);
        ui.send(Update::Delta {
            turn: id,
            text: delta.to_owned(),
        });
    };
    match generate(http, turn, model, &mut on_delta).await {
        Ok(()) => {
            ui.send(Update::Done { turn: id, reply });
        }
        Err(error) => {
            log::warn!("turn {id} failed: {error}");
            ui.send(Update::Failed { turn: id, error });
        }
    }
}

async fn generate(
    http: &HttpClient,
    turn: &ChatTurn,
    model: &str,
    on_delta: &mut dyn FnMut(&str),
) -> Result<(), ProviderError> {
    let started = Instant::now();
    if !model.is_empty() {
        let ollama = Ollama::new(http.clone(), ollama::BASE_URL);
        if ollama.is_running().await {
            let request = ChatRequest {
                model,
                system: &turn.system,
                messages: &turn.messages,
                tools: &[],
            };
            ollama.chat_stream(&request, on_delta).await?;
            log::info!(
                "turn {} answered by Ollama in {} ms",
                turn.turn,
                started.elapsed().as_millis()
            );
            return Ok(());
        }
        log::info!("turn {}: Ollama is not running", turn.turn);
    }
    let prompt = turn.messages.last().map_or("", |m| m.content.as_str());
    on_delta(&local_reply(prompt, sohi_win::local_time()));
    log::info!("turn {} answered locally", turn.turn);
    Ok(())
}

/// What Sohi says to `prompt` with no model at all. Commands are only
/// understood so far: running them arrives with the tools (P7).
fn local_reply(prompt: &str, now: LocalTime) -> String {
    match intent::parse(prompt) {
        None => intent::UNKNOWN_REPLY.to_owned(),
        Some(Intent::Time) => format!("It's {:02}:{:02}.", now.hour, now.minute),
        Some(_) => "Offline, and I can't run commands yet.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: LocalTime = LocalTime {
        year: 2026,
        month: 11,
        day: 24,
        weekday: 2,
        hour: 9,
        minute: 5,
    };

    #[test]
    fn local_replies() {
        assert_eq!(local_reply("What time is it?", NOW), "It's 09:05.");
        assert_eq!(local_reply("Tell me a joke", NOW), intent::UNKNOWN_REPLY);
        assert!(local_reply("open notepad", NOW).contains("can't run commands yet"));
    }
}
