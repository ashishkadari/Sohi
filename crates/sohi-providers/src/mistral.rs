//! Mistral chat completions, streamed (spec §2.3, §5.5).
//!
//! `POST /v1/chat/completions` with `"stream": true` answers with
//! server-sent events. Each `data:` line is a JSON chunk whose
//! `choices[0].delta.content` holds the next piece of text; the stream
//! ends with `data: [DONE]`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sohi_core::chat::Message;
use sohi_core::secret::Secret;

use crate::http::{Auth, HttpClient};
use crate::sse::SseParser;
use crate::{ChatRequest, ProviderError};

/// The public API.
pub const BASE_URL: &str = "https://api.mistral.ai";
/// The models list, which also serves as a probe.
pub(crate) const MODELS_PATH: &str = "/v1/models";
/// Spec §9.2: the first byte of a reply within 8 s, later chunks within 15 s.
pub const FIRST_BYTE: Duration = Duration::from_secs(8);
/// See [`FIRST_BYTE`].
pub const BETWEEN_CHUNKS: Duration = Duration::from_secs(15);

/// A Mistral chat client.
#[derive(Debug, Clone)]
pub struct Mistral {
    http: HttpClient,
    base_url: String,
}

impl Mistral {
    /// A client for the API at `base_url` (normally [`BASE_URL`]).
    #[must_use]
    pub fn new(http: HttpClient, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
        }
    }

    /// Streams a reply, calling `on_delta` with each piece of text as it
    /// arrives. Returns once the reply is complete.
    ///
    /// Dropping the future cancels the request.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] if the request fails or the stream is
    /// malformed or cut short.
    pub async fn chat_stream(
        &self,
        key: &Secret,
        request: &ChatRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderError> {
        let body = request_body(request)?;
        let url = format!("{}/v1/chat/completions", self.base_url);
        let headers = [("accept", "text/event-stream")];
        let mut response = self
            .http
            .post_json(&url, Auth::Bearer(key), &headers, body, FIRST_BYTE)
            .await?;
        let mut parser = SseParser::new();
        let mut finished = false;
        while let Some(chunk) = response.next_chunk(BETWEEN_CHUNKS).await? {
            for event in parser.push(chunk.as_ref()) {
                if event.data == "[DONE]" {
                    return Ok(());
                }
                finished |= take_chunk(&event.data, on_delta)?;
            }
        }
        if finished {
            Ok(())
        } else {
            Err(ProviderError::Offline("the reply ended early".to_owned()))
        }
    }

    /// The IDs of the models this key can use (`GET /v1/models`), waiting
    /// at most `timeout`.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] if the request fails or the answer
    /// cannot be read.
    pub async fn models(
        &self,
        key: &Secret,
        timeout: Duration,
    ) -> Result<Vec<String>, ProviderError> {
        let url = format!("{}{MODELS_PATH}", self.base_url);
        let exchange = async {
            self.http
                .get(&url, Auth::Bearer(key), timeout)
                .await?
                .bytes()
                .await
        };
        let bytes = tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| ProviderError::Offline("no model list in time".to_owned()))??;
        let list: ModelList = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::BadResponse(format!("model list: {e}")))?;
        Ok(list.data.into_iter().map(|m| m.id).collect())
    }
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
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
    let messages = system
        .into_iter()
        .chain(request.messages.iter().map(|m: &Message| WireMessage {
            role: m.role.as_str(),
            content: &m.content,
        }))
        .collect();
    let body = Body {
        model: request.model,
        messages,
        stream: true,
    };
    serde_json::to_vec(&body).map_err(|e| ProviderError::BadResponse(e.to_string()))
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct Delta {
    content: Option<Content>,
}

/// Plain text, or (for some models) a list of typed parts.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Parts(Vec<Part>),
}

#[derive(Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// Hands the text in one event to `on_delta`. Returns whether the event
/// said the reply is finished.
fn take_chunk(data: &str, on_delta: &mut dyn FnMut(&str)) -> Result<bool, ProviderError> {
    let chunk: Chunk = serde_json::from_str(data)
        .map_err(|e| ProviderError::BadResponse(format!("bad stream chunk: {e}")))?;
    let mut finished = false;
    for choice in chunk.choices {
        match choice.delta.content {
            Some(Content::Text(text)) if !text.is_empty() => on_delta(&text),
            Some(Content::Parts(parts)) => {
                // Only visible text; reasoning parts are not shown.
                for text in parts
                    .into_iter()
                    .filter(|p| p.kind == "text")
                    .filter_map(|p| p.text)
                {
                    on_delta(&text);
                }
            }
            _ => {}
        }
        finished |= choice.finish_reason.is_some();
    }
    Ok(finished)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, serve_once};

    fn chunk(text: &str) -> String {
        format!(
            "data: {{\"id\":\"x\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}},\"finish_reason\":null}}]}}\n\n",
            serde_json::to_string(text).unwrap()
        )
    }

    const FINISH: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\"}]}\n\n";

    async fn run(reply: Reply) -> (Result<Vec<String>, ProviderError>, String) {
        let server = serve_once(reply).await;
        let mistral = Mistral::new(HttpClient::new().unwrap(), server.url(""));
        let key = Secret::from_input("sk-mistral").unwrap();
        let history = [
            Message::user("Hi"),
            Message::assistant("Hey."),
            Message::user("Time?"),
        ];
        let request = ChatRequest {
            model: "mistral-small-latest",
            system: "Be brief.",
            messages: &history,
        };
        let mut deltas = Vec::new();
        let result = mistral
            .chat_stream(&key, &request, &mut |d| deltas.push(d.to_owned()))
            .await
            .map(|()| deltas);
        (result, server.request().await)
    }

    #[tokio::test]
    async fn streams_deltas_until_done() {
        let pieces = [
            chunk("It's "),
            chunk("late, "),
            chunk("Ashish."),
            FINISH.to_owned(),
            "data: [DONE]\n\n".to_owned(),
        ];
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let (result, request) = run(Reply::chunked(200, &pieces)).await;
        assert_eq!(result.unwrap(), ["It's ", "late, ", "Ashish."]);

        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(request.contains("authorization: Bearer sk-mistral\r\n"));
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["model"], "mistral-small-latest");
        assert_eq!(json["stream"], true);
        let roles: Vec<_> = json["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "user", "assistant", "user"]);
        assert_eq!(json["messages"][0]["content"], "Be brief.");
    }

    #[tokio::test]
    async fn a_finish_reason_without_done_still_completes() {
        let pieces = [chunk("ok"), FINISH.to_owned()];
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let (result, _) = run(Reply::chunked(200, &pieces)).await;
        assert_eq!(result.unwrap(), ["ok"]);
    }

    #[tokio::test]
    async fn a_stream_cut_short_is_an_error() {
        let pieces = [chunk("half a")];
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let (result, _) = run(Reply::chunked(200, &pieces)).await;
        assert!(matches!(result, Err(ProviderError::Offline(_))));
    }

    #[tokio::test]
    async fn garbage_is_a_bad_response() {
        let (result, _) = run(Reply::chunked(200, &["data: {not json\n\n"])).await;
        assert!(matches!(result, Err(ProviderError::BadResponse(_))));
    }

    #[tokio::test]
    async fn a_rejected_key_is_an_auth_error() {
        let (result, _) = run(Reply::status(401, "{\"message\":\"Unauthorized\"}")).await;
        assert_eq!(result.unwrap_err(), ProviderError::Auth(401));
    }

    #[tokio::test]
    async fn lists_model_ids() {
        let server = serve_once(Reply::status(
            200,
            r#"{"object":"list","data":[{"id":"voxtral-mini-latest","object":"model"},{"id":"mistral-small-latest"}]}"#,
        ))
        .await;
        let mistral = Mistral::new(HttpClient::new().unwrap(), server.url(""));
        let key = Secret::from_input("sk-mistral").unwrap();
        let ids = mistral.models(&key, Duration::from_secs(1)).await.unwrap();
        assert_eq!(ids, ["voxtral-mini-latest", "mistral-small-latest"]);
        let request = server.request().await;
        assert!(request.starts_with("GET /v1/models HTTP/1.1"));
        assert!(request.contains("authorization: Bearer sk-mistral\r\n"));
    }

    #[test]
    fn content_parts_keep_only_text() {
        let data = r#"{"choices":[{"delta":{"content":[{"type":"thinking","thinking":[]},{"type":"text","text":"Hi"}]},"finish_reason":null}]}"#;
        let mut got = Vec::new();
        assert!(!take_chunk(data, &mut |d| got.push(d.to_owned())).unwrap());
        assert_eq!(got, ["Hi"]);
    }
}
