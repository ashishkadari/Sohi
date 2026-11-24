//! Anthropic Messages API, streamed (spec §2.3: the online fallback).
//!
//! `POST /v1/messages` with `"stream": true` answers with server-sent
//! events. Text arrives in `content_block_delta` events whose delta is a
//! `text_delta`; `message_stop` ends the reply. Thinking blocks are not
//! shown. An `error` event can arrive mid-stream (e.g. `overloaded_error`).
//!
//! API version `2026-10-21`: no sampling parameters are sent.

mod blocks;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sohi_core::secret::Secret;

use crate::http::{Auth, HttpClient};
use crate::mistral::{BETWEEN_CHUNKS, FIRST_BYTE};
use crate::sse::SseParser;
use crate::{ChatRequest, Completion, ProviderError};
use blocks::Blocks;

/// The public API.
pub const BASE_URL: &str = "https://api.anthropic.com";
/// The `anthropic-version` header.
pub const API_VERSION: &str = "2026-10-21";
/// The output cap per reply. Replies are asked to be short, but thinking
/// counts against this too, so it is not set low.
pub const MAX_TOKENS: u32 = 16_000;

/// An Anthropic chat client.
#[derive(Debug, Clone)]
pub struct Anthropic {
    http: HttpClient,
    base_url: String,
}

impl Anthropic {
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
    /// Returns a [`ProviderError`] if the request fails, the stream is
    /// malformed or cut short, or the model declines with no text.
    pub async fn chat_stream(
        &self,
        key: &Secret,
        request: &ChatRequest<'_>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Completion, ProviderError> {
        let body = request_body(request)?;
        let url = format!("{}/v1/messages", self.base_url);
        let headers = [
            ("accept", "text/event-stream"),
            ("anthropic-version", API_VERSION),
        ];
        let auth = Auth::Header("x-api-key", key);
        let mut response = self
            .http
            .post_json(&url, auth, &headers, body, FIRST_BYTE)
            .await?;
        let mut parser = SseParser::new();
        let mut stream = StreamState::default();
        while let Some(chunk) = response.next_chunk(BETWEEN_CHUNKS).await? {
            for event in parser.push(chunk.as_ref()) {
                if stream.take(&event.data, on_delta)? {
                    return stream.finish();
                }
            }
        }
        Err(ProviderError::Offline("the reply ended early".to_owned()))
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "is_empty")]
    system: &'a str,
    messages: Vec<Value>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_empty(text: &&str) -> bool {
    text.is_empty()
}

fn request_body(request: &ChatRequest<'_>) -> Result<Vec<u8>, ProviderError> {
    let body = Body {
        model: request.model,
        max_tokens: MAX_TOKENS,
        system: request.system,
        messages: blocks::messages(request.messages),
        stream: true,
        tools: blocks::tools(request.tools)?,
    };
    serde_json::to_vec(&body).map_err(|e| ProviderError::BadResponse(e.to_string()))
}

/// One stream event. Only the fields Sohi reads; `type` picks the shape.
#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    /// Which content block a `content_block_*` event is about.
    #[serde(default)]
    index: u64,
    content_block: Option<Value>,
    delta: Option<Value>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: String,
}

/// What the stream has said so far.
#[derive(Default)]
struct StreamState {
    any_text: bool,
    stop_reason: Option<String>,
    blocks: Blocks,
}

impl StreamState {
    /// Handles one event's data. Returns whether the reply is complete.
    fn take(&mut self, data: &str, on_delta: &mut dyn FnMut(&str)) -> Result<bool, ProviderError> {
        let event: Event = serde_json::from_str(data)
            .map_err(|e| ProviderError::BadResponse(format!("bad stream event: {e}")))?;
        match event.kind.as_str() {
            "content_block_delta" => {
                let delta = event.delta.unwrap_or_default();
                let text = field(&delta, "text");
                if field(&delta, "type") == "text_delta" && !text.is_empty() {
                    self.any_text = true;
                    on_delta(text);
                }
                self.blocks.push(event.index, &delta);
            }
            "content_block_start" => {
                self.blocks
                    .start(event.index, event.content_block.unwrap_or_default());
            }
            "message_delta" => {
                let delta = event.delta.unwrap_or_default();
                if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_owned());
                }
            }
            "message_stop" => return Ok(true),
            "error" => return Err(stream_error(event.error)),
            // message_start, content_block_stop, ping, and any
            // event type added later.
            _ => {}
        }
        Ok(false)
    }

    /// The outcome of a stream that reached `message_stop`.
    fn finish(self) -> Result<Completion, ProviderError> {
        match self.stop_reason.as_deref() {
            Some("refusal") if !self.any_text => Err(ProviderError::Refused),
            // Calls that were cut off are not run.
            Some("refusal" | "max_tokens") => Ok(Completion::default()),
            _ => Ok(self.blocks.finish()),
        }
    }
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Classifies an `error` event the way an HTTP status would be.
fn stream_error(error: Option<ApiError>) -> ProviderError {
    let Some(error) = error else {
        return ProviderError::BadResponse("an error event without details".to_owned());
    };
    let status = match error.kind.as_str() {
        "authentication_error" => 401,
        "permission_error" => 403,
        "rate_limit_error" => 429,
        "api_error" => 500,
        "overloaded_error" => 529,
        _ => 400,
    };
    ProviderError::from_status(status, &format!("{}: {}", error.kind, error.message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, serve_once};
    use sohi_core::chat::Message;

    fn event(name: &str, data: &str) -> String {
        format!("event: {name}\ndata: {data}\n\n")
    }

    fn text(piece: &str) -> String {
        let data = format!(
            r#"{{"type":"content_block_delta","index":1,"delta":{{"type":"text_delta","text":{}}}}}"#,
            serde_json::to_string(piece).unwrap()
        );
        event("content_block_delta", &data)
    }

    fn start() -> Vec<String> {
        vec![
            event(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[]}}"#,
            ),
            event(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            event(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            ),
            event("ping", r#"{"type":"ping"}"#),
        ]
    }

    fn stop(reason: &str) -> Vec<String> {
        vec![
            event(
                "message_delta",
                &format!(
                    r#"{{"type":"message_delta","delta":{{"stop_reason":"{reason}"}},"usage":{{"output_tokens":9}}}}"#
                ),
            ),
            event("message_stop", r#"{"type":"message_stop"}"#),
        ]
    }

    async fn run(reply: Reply) -> (Result<Vec<String>, ProviderError>, String) {
        let server = serve_once(reply).await;
        let anthropic = Anthropic::new(HttpClient::new().unwrap(), server.url(""));
        let key = Secret::from_input("sk-ant-test").unwrap();
        let history = [
            Message::user("Hi"),
            Message::assistant("Hey."),
            Message::user("Time?"),
        ];
        let request = ChatRequest {
            model: "claude-haiku-5-5",
            system: "Be brief.",
            messages: &history,
            tools: &[],
        };
        let mut deltas = Vec::new();
        let result = anthropic
            .chat_stream(&key, &request, &mut |d| deltas.push(d.to_owned()))
            .await
            .map(|_| deltas);
        (result, server.request().await)
    }

    async fn run_events(events: &[String]) -> Result<Vec<String>, ProviderError> {
        let pieces: Vec<&str> = events.iter().map(String::as_str).collect();
        run(Reply::chunked(200, &pieces)).await.0
    }

    #[tokio::test]
    async fn streams_text_and_skips_thinking() {
        let mut events = start();
        events.extend([text("It's "), text("late.")]);
        events.extend(stop("end_turn"));
        let pieces: Vec<&str> = events.iter().map(String::as_str).collect();
        let (result, request) = run(Reply::chunked(200, &pieces)).await;
        assert_eq!(result.unwrap(), ["It's ", "late."]);

        assert!(request.starts_with("POST /v1/messages HTTP/1.1"));
        assert!(request.contains("x-api-key: sk-ant-test\r\n"));
        assert!(request.contains("anthropic-version: 2026-10-21\r\n"));
        assert!(!request.contains("authorization:"));
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["model"], "claude-haiku-5-5");
        assert_eq!(json["stream"], true);
        assert_eq!(json["max_tokens"], MAX_TOKENS);
        assert_eq!(json["system"], "Be brief.");
        assert!(json.get("temperature").is_none());
        let roles: Vec<_> = json["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
    }

    #[test]
    fn an_empty_system_prompt_is_left_out() {
        let request = ChatRequest {
            model: "m",
            system: "",
            messages: &[],
            tools: &[],
        };
        let json: serde_json::Value =
            serde_json::from_slice(&request_body(&request).unwrap()).unwrap();
        assert!(json.get("system").is_none());
    }

    #[tokio::test]
    async fn a_stream_without_message_stop_is_cut_short() {
        let mut events = start();
        events.push(text("half a"));
        let result = run_events(&events).await;
        assert!(matches!(result, Err(ProviderError::Offline(_))));
    }

    #[tokio::test]
    async fn an_overloaded_event_is_a_server_error() {
        let mut events = start();
        events.push(event(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ));
        assert_eq!(run_events(&events).await, Err(ProviderError::Server(529)));
    }

    #[tokio::test]
    async fn a_refusal_with_no_text_is_refused() {
        let mut events = start();
        events.extend(stop("refusal"));
        assert_eq!(run_events(&events).await, Err(ProviderError::Refused));
    }

    #[tokio::test]
    async fn a_refusal_after_some_text_keeps_the_text() {
        let mut events = start();
        events.push(text("I'll stop there."));
        events.extend(stop("refusal"));
        assert_eq!(run_events(&events).await.unwrap(), ["I'll stop there."]);
    }

    #[tokio::test]
    async fn statuses_map_like_mistral() {
        let (result, _) = run(Reply::status(401, r#"{"type":"error"}"#)).await;
        assert_eq!(result, Err(ProviderError::Auth(401)));
        let (result, _) = run(Reply::status(529, r#"{"type":"error"}"#)).await;
        assert_eq!(result, Err(ProviderError::Server(529)));
    }

    #[test]
    fn error_events_map_to_classes() {
        let err = |kind: &str| {
            stream_error(Some(ApiError {
                kind: kind.to_owned(),
                message: String::new(),
            }))
        };
        assert_eq!(err("rate_limit_error"), ProviderError::RateLimited);
        assert_eq!(err("authentication_error"), ProviderError::Auth(401));
        assert_eq!(err("api_error"), ProviderError::Server(500));
        assert!(matches!(
            err("invalid_request_error"),
            ProviderError::BadResponse(_)
        ));
        assert!(matches!(stream_error(None), ProviderError::BadResponse(_)));
    }

    async fn called(reason: &str) -> Completion {
        let mut events = vec![
            event(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"volume","input":{}}}"#,
            ),
            event(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"action\":\"up\"}"}}"#,
            ),
        ];
        events.extend(stop(reason));
        let pieces: Vec<&str> = events.iter().map(String::as_str).collect();
        let server = serve_once(Reply::chunked(200, &pieces)).await;
        let anthropic = Anthropic::new(HttpClient::new().unwrap(), server.url(""));
        let key = Secret::from_input("sk-ant-test").unwrap();
        let spec = sohi_core::chat::ToolSpec {
            name: "volume",
            description: "Change it.",
            parameters: r#"{"type":"object"}"#,
        };
        let request = ChatRequest {
            model: "m",
            system: "",
            messages: &[Message::user("Louder")],
            tools: &[spec],
        };
        let done = anthropic.chat_stream(&key, &request, &mut |_| {}).await;
        assert!(
            server
                .request()
                .await
                .contains(r#""input_schema":{"type":"object"}"#)
        );
        done.unwrap()
    }

    #[tokio::test]
    async fn tool_calls_are_collected_unless_cut_off() {
        let done = called("tool_use").await;
        assert_eq!(done.calls[0].id, "toolu_1");
        assert_eq!(done.calls[0].arguments, r#"{"action":"up"}"#);
        assert!(done.blocks.unwrap().contains("toolu_1"));
        assert_eq!(called("max_tokens").await, Completion::default());
    }
}
