//! Voxtral text-to-speech, streamed (spec §2.3, §5.5).
//!
//! `POST /v1/audio/speech` with `"stream": true` answers with server-sent
//! events: `speech.audio.delta` carries base64 `audio_data`, and
//! `speech.audio.done` ends the stream. With `"response_format": "pcm"`
//! the audio is 32-bit little-endian float, 24 kHz mono, so no decoder is
//! needed. Base64 is decoded here: it is a few lines, not a dependency.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sohi_core::secret::Secret;

use crate::http::{Auth, HttpClient};
use crate::mistral::BASE_URL;
use crate::sse::SseParser;
use crate::{ProviderError, SpeechAudio, TtsProvider};

/// The model.
pub const MODEL: &str = "voxtral-mini-tts-3";
/// The voice used if none is configured: "Sheila", US-English, female.
pub const DEFAULT_VOICE: &str = "voice_478";
/// The sample rate of the speech.
pub const SAMPLE_RATE: u32 = 24_000;
/// Spec §9.2: the first byte within 5 s, and the same between chunks.
const TIMEOUT: Duration = Duration::from_secs(5);

/// A Voxtral speech client.
#[derive(Debug)]
pub struct MistralTts {
    http: HttpClient,
    base_url: String,
    key: Secret,
    voice: String,
    timeout: Duration,
}

impl MistralTts {
    /// A client for the API at `base_url`, speaking with `voice` (empty
    /// for [`DEFAULT_VOICE`]).
    #[must_use]
    pub fn new(http: HttpClient, base_url: impl Into<String>, key: Secret, voice: &str) -> Self {
        let voice = voice.trim();
        Self {
            http,
            base_url: base_url.into(),
            key,
            voice: if voice.is_empty() {
                DEFAULT_VOICE.to_owned()
            } else {
                voice.to_owned()
            },
            timeout: TIMEOUT,
        }
    }

    /// A client for the public API.
    #[must_use]
    pub fn public(http: HttpClient, key: Secret, voice: &str) -> Self {
        Self::new(http, BASE_URL, key, voice)
    }
}

#[derive(Serialize)]
struct Body<'a> {
    model: &'a str,
    input: &'a str,
    voice_id: &'a str,
    response_format: &'a str,
    stream: bool,
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    audio_data: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl TtsProvider for MistralTts {
    async fn speak(
        &self,
        text: &str,
        on_audio: &mut dyn FnMut(SpeechAudio<'_>),
    ) -> Result<(), ProviderError> {
        let body = serde_json::to_vec(&Body {
            model: MODEL,
            input: text,
            voice_id: &self.voice,
            response_format: "pcm",
            stream: true,
        })
        .map_err(|e| ProviderError::BadResponse(format!("request: {e}")))?;
        let url = format!("{}/v1/audio/speech", self.base_url);
        let headers = [("accept", "text/event-stream")];
        let mut response = self
            .http
            .post_json(&url, Auth::Bearer(&self.key), &headers, body, self.timeout)
            .await?;
        let mut parser = SseParser::new();
        let mut pcm = FloatPcm::default();
        while let Some(chunk) = response.next_chunk(self.timeout).await? {
            for event in parser.push(chunk.as_ref()) {
                let event: Event = serde_json::from_str(&event.data)
                    .map_err(|e| ProviderError::BadResponse(format!("speech event: {e}")))?;
                if event.kind == "speech.audio.done" {
                    return Ok(());
                }
                if let Some(audio) = event.audio_data {
                    pcm.push(&audio)?;
                    if !pcm.samples.is_empty() {
                        on_audio(SpeechAudio {
                            sample_rate: SAMPLE_RATE,
                            samples: &pcm.samples,
                        });
                    }
                } else if event.kind == "error" {
                    let message = event.message.unwrap_or_default();
                    return Err(ProviderError::BadResponse(format!("speech: {message}")));
                }
            }
        }
        Err(ProviderError::Offline("the speech ended early".to_owned()))
    }
}

/// Turns base64 float samples into 16-bit ones.
#[derive(Default)]
struct FloatPcm {
    bytes: Vec<u8>,
    samples: Vec<i16>,
}

impl FloatPcm {
    /// Decodes the next piece into `samples`. A sample split across two
    /// pieces is completed by the second.
    fn push(&mut self, base64: &str) -> Result<(), ProviderError> {
        decode_base64(base64, &mut self.bytes)
            .ok_or_else(|| ProviderError::BadResponse("speech is not base64".to_owned()))?;
        self.samples.clear();
        let (whole, _) = self.bytes.as_chunks::<4>();
        self.samples
            .extend(whole.iter().map(|b| to_i16(f32::from_le_bytes(*b))));
        let used = whole.len() * 4;
        self.bytes.drain(..used);
        Ok(())
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "clamped to the i16 range; NaN becomes 0"
)]
fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32_767.0) as i16
}

/// Appends the bytes of standard base64 `text` to `out`, or gives `None`
/// if it is malformed.
fn decode_base64(text: &str, out: &mut Vec<u8>) -> Option<()> {
    let text = text.trim_end_matches('=').as_bytes();
    if text.len() % 4 == 1 {
        return None;
    }
    out.reserve(text.len() / 4 * 3 + 2);
    for group in text.chunks(4) {
        let mut bits = 0u32;
        for &c in group {
            bits = bits << 6 | u32::from(base64_value(c)?);
        }
        bits <<= 6 * (4 - group.len());
        let [_, bytes @ ..] = bits.to_be_bytes();
        out.extend_from_slice(bytes.get(..group.len() - 1)?);
    }
    Some(())
}

fn base64_value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, serve_once};

    fn client(server_url: &str, voice: &str) -> MistralTts {
        let key = Secret::from_input("sk-voice").unwrap();
        MistralTts::new(HttpClient::new().unwrap(), server_url, key, voice)
    }

    #[test]
    fn base64_round_trips() {
        for (text, bytes) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYmFy", b"foobar"),
            ("+/8A", &[0xFB, 0xFF, 0x00]),
        ] {
            let mut out = Vec::new();
            decode_base64(text, &mut out).unwrap();
            assert_eq!(out, bytes, "{text}");
        }
        for bad in ["A", "Zm9vY", "Zm 9", "Zm-_"] {
            assert!(decode_base64(bad, &mut Vec::new()).is_none(), "{bad}");
        }
    }

    #[test]
    fn floats_become_clamped_samples() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32_767);
        assert_eq!(to_i16(-7.0), -32_767);
        assert_eq!(to_i16(f32::NAN), 0);
    }

    #[test]
    fn a_sample_split_across_pieces_is_kept() {
        // 0.5 and -0.5 are 00 00 00 3F and 00 00 00 BF, split 3 + 5.
        let mut pcm = FloatPcm::default();
        pcm.push("AAAA").unwrap();
        assert_eq!(pcm.samples, [0i16; 0]);
        pcm.push("PwAAAL8=").unwrap();
        assert_eq!(pcm.samples, [16_383, -16_383]);
    }

    #[tokio::test]
    async fn streams_the_samples() {
        let server = serve_once(Reply::chunked(
            200,
            &[
                "event: speech.audio.delta\ndata: {\"type\":\"speech.audio.delta\",\"audio_da",
                "ta\":\"AAAAPw==\"}\n\nevent: speech.audio.delta\ndata: {\"type\":\"speech.audio.delta\",\"audio_data\":\"AAAAvw==\"}\n\n",
                "event: speech.audio.done\ndata: {\"type\":\"speech.audio.done\",\"usage\":{\"prompt_tokens\":19}}\n\n",
            ],
        ))
        .await;
        let mut got = Vec::new();
        client(&server.url(""), " ")
            .speak("Hi.", &mut |audio| {
                assert_eq!(audio.sample_rate, 24_000);
                got.extend_from_slice(audio.samples);
            })
            .await
            .unwrap();
        assert_eq!(got, [16_383, -16_383]);
        let raw = server.request().await;
        assert!(raw.starts_with("POST /v1/audio/speech HTTP/1.1\r\n"));
        assert!(raw.contains("authorization: Bearer sk-voice\r\n"));
        assert!(raw.ends_with(
            r#"{"model":"voxtral-mini-tts-3","input":"Hi.","voice_id":"voice_478","response_format":"pcm","stream":true}"#
        ));
    }

    #[tokio::test]
    async fn failures_are_classified() {
        let mut ignore = |_: SpeechAudio<'_>| ();
        let server = serve_once(Reply::status(401, "{}")).await;
        let got = client(&server.url(""), "v").speak("x", &mut ignore).await;
        assert_eq!(got, Err(ProviderError::Auth(401)));

        let cut = "data: {\"type\":\"speech.audio.delta\",\"audio_data\":\"AAAAPw==\"}\n\n";
        let server = serve_once(Reply::chunked(200, &[cut])).await;
        let got = client(&server.url(""), "v").speak("x", &mut ignore).await;
        assert!(matches!(got, Err(ProviderError::Offline(_))), "{got:?}");

        let bad = "data: {\"type\":\"speech.audio.delta\",\"audio_data\":\"?\"}\n\n";
        let server = serve_once(Reply::chunked(200, &[bad])).await;
        let got = client(&server.url(""), "v").speak("x", &mut ignore).await;
        assert!(matches!(got, Err(ProviderError::BadResponse(_))), "{got:?}");

        let server = serve_once(Reply::stalled(200, cut)).await;
        let mut stalled = client(&server.url(""), "v");
        stalled.timeout = Duration::from_millis(200);
        let got = stalled.speak("x", &mut ignore).await;
        assert!(matches!(got, Err(ProviderError::Offline(_))), "{got:?}");
    }
}
