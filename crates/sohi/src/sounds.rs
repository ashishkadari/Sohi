//! Plays the sounds (spec §5.11).

use std::time::Instant;

use sohi_audio::playback::playback_clip;
use sohi_audio::sound::{SAMPLE_RATE, Sound};
use sohi_core::config::SoundsConfig;
use sohi_core::interaction::State;
use sohi_win::Playback;

/// Plays one sound at a time.
pub struct Sounds {
    config: SoundsConfig,
    /// The latest one. Its thread ends by itself.
    playing: Option<Playback>,
}

impl Sounds {
    /// Silent until asked.
    pub fn new(config: SoundsConfig) -> Self {
        Self {
            config,
            playing: None,
        }
    }

    /// Plays `sound` instead of whatever is playing.
    pub fn play(&mut self, sound: Sound) {
        if !self.config.enabled || self.config.volume == 0 {
            return;
        }
        if let Some(took) = self.playing.take().and_then(|p| p.first_audio_micros()) {
            log::debug!("the last sound began after {} ms", took / 1_000);
        }
        let samples = crate::speak::audible(&sound.samples(self.config.volume));
        let clip = playback_clip(&samples, SAMPLE_RATE);
        self.playing = Playback::open(clip, Instant::now())
            .inspect_err(|err| log::warn!("cannot play a sound: {err}"))
            .ok();
        log::debug!("sound: {sound:?}");
    }
}

/// The sound for a change of state. `spoken` says whether it is a voice
/// turn.
pub fn cue(before: State, after: State, spoken: bool) -> Option<Sound> {
    match (before, after) {
        (State::Listening { turn: a }, State::Listening { turn: b }) if a == b => None,
        (_, State::Listening { .. }) => Some(Sound::Listen),
        (State::Listening { .. }, State::Transcribing { .. }) => Some(Sound::Heard),
        (State::Error, State::Error) | (State::Confirming { .. }, State::Confirming { .. }) => None,
        (_, State::Error) => Some(Sound::Error),
        // In a voice turn the mic opens for the answer.
        (_, State::Confirming { .. }) if !spoken => Some(Sound::Ask),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_moment_has_its_sound() {
        let listening = |turn| State::Listening { turn };
        let confirming = |ask| State::Confirming { turn: 1, ask };
        let transcribing = State::Transcribing { turn: 1 };
        let thinking = State::Thinking { turn: 1 };
        let speaking = State::Speaking { turn: 1 };
        for (before, after, want) in [
            (State::Idle, listening(1), Some(Sound::Listen)),
            (listening(1), listening(1), None),
            (listening(1), listening(2), Some(Sound::Listen)),
            (speaking, listening(2), Some(Sound::Listen)),
            (listening(1), transcribing, Some(Sound::Heard)),
            (listening(1), State::Idle, None),
            (thinking, State::Error, Some(Sound::Error)),
            (State::Error, State::TextEntry, None),
            (thinking, confirming(1), Some(Sound::Ask)),
            (confirming(1), confirming(1), None),
            (confirming(1), thinking, None),
            (State::Idle, State::TextEntry, None),
            (thinking, speaking, None),
        ] {
            assert_eq!(cue(before, after, false), want, "{before:?} {after:?}");
        }
        assert_eq!(cue(thinking, confirming(1), true), None);
        assert_eq!(cue(State::Idle, listening(1), true), Some(Sound::Listen));
    }
}
