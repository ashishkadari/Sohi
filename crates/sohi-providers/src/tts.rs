//! Text-to-speech (spec §2.3, §5.5).

use std::future::Future;

use crate::ProviderError;

/// A piece of synthesized speech: mono 16-bit samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechAudio<'a> {
    /// Samples per second; the same for every piece of one sentence.
    pub sample_rate: u32,
    /// The samples, in playing order.
    pub samples: &'a [i16],
}

/// Turns text into speech.
pub trait TtsProvider {
    /// Synthesizes `text` (one sentence, already free of markup), calling
    /// `on_audio` with each piece as it arrives. Returns once all of it
    /// has been delivered. Dropping the future cancels the request.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] if synthesis fails or is cut short;
    /// pieces delivered before that are valid.
    fn speak(
        &self,
        text: &str,
        on_audio: &mut dyn FnMut(SpeechAudio<'_>),
    ) -> impl Future<Output = Result<(), ProviderError>>;
}
