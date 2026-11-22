//! STT, LLM and TTS provider traits and their Mistral, Anthropic, Ollama and `OneCore` implementations.

#![forbid(unsafe_code)]

mod error;
pub mod http;
#[cfg(test)]
mod testserver;

pub use error::ProviderError;
pub use http::{Auth, HttpClient};
