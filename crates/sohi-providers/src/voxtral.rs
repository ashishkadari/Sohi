//! Voxtral speech-to-text (spec §2.3, §5.5).
//!
//! `POST /v1/audio/transcriptions` takes `multipart/form-data` with the
//! audio in `file`, the `model`, and an optional ISO 639-1 `language`
//! (left out to detect it). The answer is JSON whose `text` is the
//! transcript. The form is built by hand: it is three small parts, and
//! reqwest's multipart support would add a dependency for no gain.

use std::time::Duration;

use serde::Deserialize;
use sohi_core::secret::Secret;

use crate::ProviderError;
use crate::http::{Auth, HttpClient};
use crate::mistral::BASE_URL;

/// Spec §5.5: a transcription gets at most 10 s, upload included.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// What to transcribe.
#[derive(Debug, Clone, Copy)]
pub struct TranscribeRequest<'a> {
    /// The model, such as `voxtral-mini-latest`.
    pub model: &'a str,
    /// An ISO 639-1 code, or `auto` (or empty) to let the service detect it.
    pub language: &'a str,
    /// The audio as a WAV file.
    pub wav: &'a [u8],
}

/// A transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    /// What was said, trimmed; empty if nothing was.
    pub text: String,
    /// The language the service heard, if it says.
    pub language: Option<String>,
}

/// A Voxtral client.
#[derive(Debug, Clone)]
pub struct Voxtral {
    http: HttpClient,
    base_url: String,
    timeout: Duration,
}

impl Voxtral {
    /// A client for the API at `base_url` (normally [`BASE_URL`]).
    #[must_use]
    pub fn new(http: HttpClient, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            timeout: TIMEOUT,
        }
    }

    /// A client for the public API.
    #[must_use]
    pub fn public(http: HttpClient) -> Self {
        Self::new(http, BASE_URL)
    }

    /// Transcribes `request.wav`. Dropping the future cancels the upload.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] if the request fails, takes longer
    /// than [`TIMEOUT`], or the answer cannot be read.
    pub async fn transcribe(
        &self,
        key: &Secret,
        request: &TranscribeRequest<'_>,
    ) -> Result<Transcript, ProviderError> {
        let boundary = boundary(u64::from(crate::retry::jitter_seed()), request.wav);
        let body = form(&boundary, request);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let url = format!("{}/v1/audio/transcriptions", self.base_url);
        let exchange = async {
            let response = self
                .http
                .post(
                    &url,
                    Auth::Bearer(key),
                    &content_type,
                    &[],
                    body,
                    self.timeout,
                )
                .await?;
            response.bytes().await
        };
        let bytes = tokio::time::timeout(self.timeout, exchange)
            .await
            .map_err(|_| ProviderError::Offline("no transcript in time".to_owned()))??;
        parse(&bytes)
    }
}

/// A boundary that does not occur in `data`. Starting from `seed`, it
/// tries again in the (vanishingly rare) case that it does.
fn boundary(mut seed: u64, data: &[u8]) -> String {
    loop {
        let candidate = format!("sohi-{seed:016x}");
        if !data
            .windows(candidate.len())
            .any(|w| w == candidate.as_bytes())
        {
            return candidate;
        }
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    }
}

/// The `multipart/form-data` body.
fn form(boundary: &str, request: &TranscribeRequest<'_>) -> Vec<u8> {
    let mut body = Vec::with_capacity(request.wav.len() + 512);
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    field("model", request.model);
    let language = request.language.trim();
    if !language.is_empty() && !language.eq_ignore_ascii_case("auto") {
        field("language", language);
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(request.wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[derive(Deserialize)]
struct Answer {
    text: String,
    #[serde(default)]
    language: Option<String>,
}

fn parse(bytes: &[u8]) -> Result<Transcript, ProviderError> {
    let answer: Answer = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::BadResponse(format!("transcript: {e}")))?;
    Ok(Transcript {
        text: answer.text.trim().to_owned(),
        language: answer.language.filter(|l| !l.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, serve_once};

    fn key() -> Secret {
        Secret::from_input("sk-voice").unwrap()
    }

    fn client(server_url: &str) -> Voxtral {
        Voxtral::new(HttpClient::new().unwrap(), server_url)
    }

    const WAV: &[u8] = b"RIFF\x10\0\0\0WAVEfake audio";

    #[tokio::test]
    async fn uploads_a_multipart_form_and_reads_the_text() {
        let server = serve_once(Reply::status(
            200,
            r#"{"model":"voxtral-mini-latest","text":" Open the browser. ","language":"en"}"#,
        ))
        .await;
        let request = TranscribeRequest {
            model: "voxtral-mini-latest",
            language: "en",
            wav: WAV,
        };
        let got = client(&server.url(""))
            .transcribe(&key(), &request)
            .await
            .unwrap();
        assert_eq!(got.text, "Open the browser.");
        assert_eq!(got.language.as_deref(), Some("en"));

        let raw = server.request().await;
        assert!(raw.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"));
        assert!(raw.contains("authorization: Bearer sk-voice\r\n"));
        let marker = "content-type: multipart/form-data; boundary=";
        let start = raw.find(marker).unwrap() + marker.len();
        let boundary = &raw[start..start + raw[start..].find("\r\n").unwrap()];
        let body = &raw[raw.find("\r\n\r\n").unwrap() + 4..];
        let want = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nvoxtral-mini-latest\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\nen\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\nRIFF\u{10}\0\0\0WAVEfake audio\r\n--{boundary}--\r\n"
        );
        assert_eq!(body, want);
    }

    #[test]
    fn auto_language_is_left_out() {
        for language in ["auto", "AUTO", "", "  "] {
            let request = TranscribeRequest {
                model: "m",
                language,
                wav: WAV,
            };
            let body = String::from_utf8_lossy(&form("b", &request)).into_owned();
            assert!(!body.contains("name=\"language\""), "{language:?}");
            assert!(body.contains("name=\"model\""));
        }
    }

    #[test]
    fn the_boundary_never_occurs_in_the_audio() {
        let first = boundary(7, b"");
        let mut audio = b"xx".to_vec();
        audio.extend_from_slice(first.as_bytes());
        let second = boundary(7, &audio);
        assert_ne!(first, second);
        assert!(second.starts_with("sohi-"));
    }

    #[tokio::test]
    async fn errors_are_classified() {
        let server = serve_once(Reply::status(401, r#"{"message":"Unauthorized"}"#)).await;
        let request = TranscribeRequest {
            model: "m",
            language: "auto",
            wav: WAV,
        };
        let got = client(&server.url(""))
            .transcribe(&key(), &request)
            .await
            .unwrap_err();
        assert_eq!(got, ProviderError::Auth(401));

        let server = serve_once(Reply::status(200, "not json")).await;
        let got = client(&server.url(""))
            .transcribe(&key(), &request)
            .await
            .unwrap_err();
        assert!(matches!(got, ProviderError::BadResponse(_)), "{got:?}");
    }

    #[tokio::test]
    async fn a_stalled_answer_times_out_as_offline() {
        let server = serve_once(Reply::stalled(200, r#"{"text":"#)).await;
        let mut voxtral = client(&server.url(""));
        voxtral.timeout = Duration::from_millis(200);
        let request = TranscribeRequest {
            model: "m",
            language: "auto",
            wav: WAV,
        };
        let got = voxtral.transcribe(&key(), &request).await.unwrap_err();
        assert!(matches!(got, ProviderError::Offline(_)), "{got:?}");
    }

    #[test]
    fn silence_gives_an_empty_transcript() {
        let got = parse(br#"{"text":"","language":null}"#).unwrap();
        assert_eq!(
            got,
            Transcript {
                text: String::new(),
                language: None
            }
        );
    }
}
