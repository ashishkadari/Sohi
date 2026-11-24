//! STT, LLM and TTS provider traits and their Mistral, Anthropic, Ollama and `OneCore` implementations.

#![forbid(unsafe_code)]

pub mod anthropic;
mod error;
pub mod http;
pub mod mistral;
pub mod mistral_tts;
pub mod ollama;
pub mod probe;
pub mod retry;
pub mod sse;
#[cfg(test)]
mod testserver;
pub mod tts;
pub mod voxtral;

use sohi_core::chat::{Message, ToolCall, ToolSpec};

pub use anthropic::Anthropic;
pub use error::ProviderError;
pub use http::{Auth, HttpClient};
pub use mistral::Mistral;
pub use mistral_tts::MistralTts;
pub use ollama::Ollama;
pub use tts::{SpeechAudio, TtsProvider};
pub use voxtral::Voxtral;

/// What a chat provider is asked.
#[derive(Debug, Clone, Copy)]
pub struct ChatRequest<'a> {
    /// The provider's model name.
    pub model: &'a str,
    /// The system prompt; empty for none.
    pub system: &'a str,
    /// The conversation so far, oldest first, ending with the user's turn.
    pub messages: &'a [Message],
    /// The tools the model may call.
    pub tools: &'a [ToolSpec],
}

/// How a streamed reply ended, besides its text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    /// The tools the model wants called. Empty: the reply is complete.
    pub calls: Vec<ToolCall>,
    /// See [`Message::blocks`].
    pub blocks: Option<String>,
}
