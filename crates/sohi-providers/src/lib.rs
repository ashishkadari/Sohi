//! STT, LLM and TTS provider traits and their Mistral, Anthropic, Ollama and `OneCore` implementations.

#![forbid(unsafe_code)]

pub mod anthropic;
mod error;
pub mod http;
pub mod mistral;
pub mod sse;
#[cfg(test)]
mod testserver;

use sohi_core::chat::Message;

pub use anthropic::Anthropic;
pub use error::ProviderError;
pub use http::{Auth, HttpClient};
pub use mistral::Mistral;

/// What a chat provider is asked.
#[derive(Debug, Clone, Copy)]
pub struct ChatRequest<'a> {
    /// The provider's model name.
    pub model: &'a str,
    /// The system prompt; empty for none.
    pub system: &'a str,
    /// The conversation so far, oldest first, ending with the user's turn.
    pub messages: &'a [Message],
}
