//! Typed turns while offline (spec §5.3): a local Ollama model if one is
//! configured and running, otherwise the command grammar.

use std::time::Instant;

use sohi_core::chat::without_calls;
use sohi_core::intent::{self, Intent, Media, Volume};
use sohi_core::persona::LocalTime;
use sohi_core::reminder::{Phrase, Repeat};
use sohi_providers::{ChatRequest, HttpClient, Ollama, ProviderError, ollama};
use sohi_tools::{Call, MediaAction, VolumeAction};

use crate::actions::{Actions, NoEar, Toolbox};
use crate::chat::{ChatTurn, Update};
use crate::runtime::UiLink;

/// Answers `turn` without the internet, using `model` if possible.
pub async fn answer(
    http: &HttpClient,
    turn: &ChatTurn,
    model: &str,
    toolbox: &Toolbox,
    ui: &UiLink<Update>,
) {
    let id = turn.turn;
    let mut reply = String::new();
    let mut on_delta = |delta: &str| {
        reply.push_str(delta);
        ui.send(Update::Delta {
            turn: id,
            text: delta.to_owned(),
        });
    };
    let mut actions = Actions::new(id, &turn.policy, toolbox, ui, NoEar);
    match generate(http, turn, model, &mut actions, &mut on_delta).await {
        Ok(()) => {
            let runs = Vec::new();
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

async fn generate(
    http: &HttpClient,
    turn: &ChatTurn,
    model: &str,
    actions: &mut Actions<'_, NoEar>,
    on_delta: &mut dyn FnMut(&str),
) -> Result<(), ProviderError> {
    let started = Instant::now();
    // A command is done at once, whatever model there is: it would only be
    // slower, and has no tools.
    let prompt = turn.messages.last().map_or("", |m| m.content.as_str());
    let command = intent::parse(prompt);
    if command.is_none() && !model.is_empty() {
        let ollama = Ollama::new(http.clone(), ollama::BASE_URL);
        if ollama.is_running().await {
            let messages = without_calls(&turn.messages);
            let request = ChatRequest {
                model,
                system: &turn.system,
                messages: &messages,
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
    on_delta(&match command.map(Command::from) {
        None => intent::UNKNOWN_REPLY.to_owned(),
        Some(Command::Time) => say_time(sohi_win::local_time()),
        Some(Command::Tool(call)) => actions.execute(call).await.message,
    });
    log::info!("turn {} answered locally", turn.turn);
    Ok(())
}

/// What a command does.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Time,
    Tool(Call),
}

impl From<Intent> for Command {
    fn from(intent: Intent) -> Self {
        let media = |action| Self::Tool(Call::Media(action));
        let volume = |action| Self::Tool(Call::Volume(action));
        match intent {
            Intent::Time => Self::Time,
            Intent::Open(name) => Self::Tool(Call::LaunchApp { name }),
            Intent::GoTo(url) => Self::Tool(Call::OpenUrl { url }),
            Intent::Copy(text) => Self::Tool(Call::ClipboardSet { text }),
            Intent::Media(Media::Play) => media(MediaAction::Play),
            Intent::Media(Media::Pause) => media(MediaAction::Pause),
            Intent::Media(Media::Next) => media(MediaAction::Next),
            Intent::Media(Media::Previous) => media(MediaAction::Previous),
            Intent::Mute => media(MediaAction::Mute),
            Intent::Volume(Volume::Up) => volume(VolumeAction::Up),
            Intent::Volume(Volume::Down) => volume(VolumeAction::Down),
            Intent::Reminder(Phrase::Set(text, when)) => Self::Tool(Call::ReminderSet {
                text,
                when,
                repeat: Repeat::Once,
            }),
            Intent::Reminder(Phrase::List) => Self::Tool(Call::ReminderList),
            Intent::Reminder(Phrase::Cancel(id)) => Self::Tool(Call::ReminderCancel { id }),
            Intent::Volume(Volume::Set(level)) => volume(VolumeAction::Set(level)),
        }
    }
}

fn say_time(now: LocalTime) -> String {
    format!("It's {:02}:{:02}.", now.hour, now.minute)
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

    fn command(prompt: &str) -> Option<Command> {
        intent::parse(prompt).map(Command::from)
    }

    #[test]
    fn commands_become_tool_calls() {
        assert_eq!(say_time(NOW), "It's 09:05.");
        assert_eq!(command("What time is it?"), Some(Command::Time));
        assert_eq!(command("Tell me a joke"), None);
        for (prompt, want) in [
            (
                "open notepad",
                Call::LaunchApp {
                    name: "notepad".to_owned(),
                },
            ),
            (
                "go to github.com",
                Call::OpenUrl {
                    url: "github.com".to_owned(),
                },
            ),
            (
                "copy hello",
                Call::ClipboardSet {
                    text: "hello".to_owned(),
                },
            ),
            ("pause", Call::Media(MediaAction::Pause)),
            ("mute", Call::Media(MediaAction::Mute)),
            ("volume 30", Call::Volume(VolumeAction::Set(30))),
        ] {
            assert_eq!(command(prompt), Some(Command::Tool(want)), "{prompt}");
        }
    }
}
