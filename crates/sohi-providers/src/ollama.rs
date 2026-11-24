//! A local Ollama (spec §2.3, §5.3): the model used while offline.
//!
//! Only ever contacted on `127.0.0.1`. `GET /api/tags` says whether it
//! runs; `POST /api/chat` with `"stream": true` answers with NDJSON: one
//! JSON object per line, whose `message.content` holds the next piece of
//! text, until one has `"done": true`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::http::{Auth, HttpClient};
use crate::{ChatRequest, ProviderError};

/// Where Ollama listens.
pub const BASE_URL: &str = "http://127.0.0.1:11434";
/// Spec §5.3: Ollama counts as running if it answers this fast.
const DETECT: Duration = Duration::from_millis(150);
/// The first token waits for the model to be loaded from disk.
const FIRST_BYTE: Duration = Duration::from_mins(1);
/// Between pieces of the reply.
const BETWEEN_CHUNKS: Duration = Duration::from_secs(30);
/// A line longer than this is not a chat chunk.
const MAX_LINE: usize = 1 << 20;

/// An Ollama client.
#[derive(Debug, Clone)]
pub struct Ollama {
    http: HttpClient,
    base_url: String,
}

impl Ollama {
    /// A client for the Ollama at `base_url` (normally [`BASE_URL`]).
    #[must_use]
    pub fn new(http: HttpClient, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
        }
    }

    /// Whether Ollama is running.
    pub async fn is_running(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        self.http.get(&url, Auth::None, DETECT).await.is_ok()
    }

    /// Streams a reply, calling `on_delta` with each piece of text as it
    /// arrives. Dropping the future cancels the request.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] if the request fails, Ollama reports an
    /// error (such as a model that is not installed), or the stream is
    /// malformed or cut short.
    pub async fn chat_stream(
        &self,
        request: &ChatRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderError> {
        let body = request_body(request)?;
        let url = format!("{}/api/chat", self.base_url);
        let mut response = self
            .http
            .post_json(&url, Auth::None, &[], body, FIRST_BYTE)
            .await?;
        let mut lines = Lines::default();
        while let Some(chunk) = response.next_chunk(BETWEEN_CHUNKS).await? {
            lines.push(chunk.as_ref())?;
            while let Some(line) = lines.next() {
                if take_line(&line, on_delta)? {
                    return Ok(());
                }
            }
        }
        Err(ProviderError::Offline("the reply ended early".to_owned()))
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

fn request_body(request: &ChatRequest<'_>) -> Result<Vec<u8>, ProviderError> {
    let system = (!request.system.is_empty()).then_some(WireMessage {
        role: "system",
        content: request.system,
    });
    let messages = request.messages.iter().map(|m| WireMessage {
        role: m.role.as_str(),
        content: &m.content,
    });
    serde_json::to_vec(&Body {
        model: request.model,
        messages: system.into_iter().chain(messages).collect(),
        stream: true,
    })
    .map_err(|e| ProviderError::BadResponse(format!("request: {e}")))
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: String,
}

/// Handles one line; true if it was the last.
fn take_line(line: &[u8], on_delta: &mut dyn FnMut(&str)) -> Result<bool, ProviderError> {
    if line.trim_ascii().is_empty() {
        return Ok(false);
    }
    let chunk: Chunk = serde_json::from_slice(line)
        .map_err(|e| ProviderError::BadResponse(format!("chat line: {e}")))?;
    if let Some(error) = chunk.error {
        return Err(ProviderError::BadResponse(format!("Ollama: {error}")));
    }
    if let Some(message) = chunk.message
        && !message.content.is_empty()
    {
        on_delta(&message.content);
    }
    Ok(chunk.done)
}

/// Cuts bytes that arrive split anywhere into lines.
#[derive(Default)]
struct Lines {
    bytes: Vec<u8>,
}

impl Lines {
    fn push(&mut self, bytes: &[u8]) -> Result<(), ProviderError> {
        self.bytes.extend_from_slice(bytes);
        if self.bytes.len() > MAX_LINE && !self.bytes.contains(&b'\n') {
            return Err(ProviderError::BadResponse("endless chat line".to_owned()));
        }
        Ok(())
    }

    /// The next complete line.
    fn next(&mut self) -> Option<Vec<u8>> {
        let end = self.bytes.iter().position(|&b| b == b'\n')?;
        let mut line: Vec<u8> = self.bytes.drain(..=end).collect();
        line.pop();
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use sohi_core::chat::Message;

    use super::*;
    use crate::testserver::{Reply, closed_port_url, serve_once};

    fn ollama(url: String) -> Ollama {
        Ollama::new(HttpClient::new().unwrap(), url)
    }

    /// Asks "hi"; returns the reply.
    async fn ask(url: String) -> (Result<(), ProviderError>, String) {
        let messages = [Message::user("hi".to_owned())];
        let request = ChatRequest {
            model: "tiny",
            system: "Be brief.",
            messages: &messages,
        };
        let mut reply = String::new();
        let result = ollama(url)
            .chat_stream(&request, &mut |d| reply.push_str(d))
            .await;
        (result, reply)
    }

    #[tokio::test]
    async fn streams_ndjson_split_anywhere() {
        let server = serve_once(Reply::chunked(
            200,
            &[
                "{\"message\":{\"role\":\"assistant\",\"content\":\"He\"},\"done\":false}\n{\"mess",
                "age\":{\"content\":\"llo\"},\"done\":false}\n\n",
                "{\"message\":{\"content\":\"\"},\"done\":true,\"total_duration\":5}\n",
            ],
        ))
        .await;
        let (result, reply) = ask(server.url("")).await;
        assert_eq!(result, Ok(()));
        assert_eq!(reply, "Hello");
        let raw = server.request().await;
        assert!(raw.starts_with("POST /api/chat HTTP/1.1\r\n"));
        assert!(!raw.contains("authorization"));
        assert!(raw.ends_with(
            r#"{"model":"tiny","messages":[{"role":"system","content":"Be brief."},{"role":"user","content":"hi"}],"stream":true}"#
        ));
    }

    #[tokio::test]
    async fn failures_are_classified() {
        let server = serve_once(Reply::chunked(
            200,
            &["{\"error\":\"model 'tiny' not found\"}\n"],
        ))
        .await;
        let (result, _) = ask(server.url("")).await;
        assert!(matches!(result, Err(ProviderError::BadResponse(_))));

        let server = serve_once(Reply::chunked(200, &["nonsense\n"])).await;
        let (result, _) = ask(server.url("")).await;
        assert!(matches!(result, Err(ProviderError::BadResponse(_))));

        let cut = "{\"message\":{\"content\":\"Hel\"},\"done\":false}\n";
        let server = serve_once(Reply::chunked(200, &[cut])).await;
        let (result, reply) = ask(server.url("")).await;
        assert!(matches!(result, Err(ProviderError::Offline(_))));
        assert_eq!(reply, "Hel");
    }

    #[tokio::test]
    async fn detects_whether_it_runs() {
        let server = serve_once(Reply::status(200, r#"{"models":[]}"#)).await;
        assert!(ollama(server.url("")).is_running().await);
        assert!(!ollama(closed_port_url().await).is_running().await);
    }
}
