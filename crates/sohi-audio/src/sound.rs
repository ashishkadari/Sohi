//! Sohi's sounds (spec §5.11), synthesized: there are no sound files.

use std::f32::consts::TAU;

/// The rate the sounds are made at, in Hz.
pub const SAMPLE_RATE: u32 = 24_000;
/// How loud full volume is: well below full scale.
const FULL: f32 = 0.5;

/// A sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// The mic opened.
    Listen,
    /// The utterance ended.
    Heard,
    /// A tool question is waiting for an answer.
    Ask,
    /// Something went wrong.
    Error,
    /// A reminder is due.
    Reminder,
    /// The cape takes the screen over.
    CapeOver,
    /// The cape goes back.
    CapeBack,
}

impl Sound {
    /// Every sound.
    pub const ALL: [Self; 7] = [
        Self::Listen,
        Self::Heard,
        Self::Ask,
        Self::Error,
        Self::Reminder,
        Self::CapeOver,
        Self::CapeBack,
    ];

    /// The sound's samples, at `volume` percent.
    #[must_use]
    pub fn samples(self, volume: u8) -> Vec<i16> {
        let wave = match self {
            Self::Listen => notes(&[(1760.0, 0, 50), (2637.0, 50, 90)], 35.0),
            Self::Heard => notes(&[(880.0, 0, 110)], 40.0),
            Self::Ask => notes(
                &[(740.0, 0, 120), (740.0, 140, 120), (987.8, 280, 260)],
                80.0,
            ),
            Self::Error => notes(&[(329.6, 0, 160), (246.9, 160, 320)], 120.0),
            Self::Reminder => notes(
                &[(1046.5, 0, 500), (1318.5, 160, 600), (1568.0, 320, 900)],
                280.0,
            ),
            Self::CapeOver => whoosh(1_700, true),
            Self::CapeBack => whoosh(800, false),
        };
        let loudest = wave.iter().fold(f32::EPSILON, |max, s| max.max(s.abs()));
        let gain = f32::from(volume.min(100)) / 100.0 * FULL * self.level() / loudest;
        wave.iter().map(|s| sample(s * gain)).collect()
    }

    /// How loud it is among the others.
    fn level(self) -> f32 {
        match self {
            Self::Heard | Self::Listen => 0.7,
            Self::Ask | Self::Error => 0.8,
            Self::CapeOver | Self::CapeBack | Self::Reminder => 1.0,
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "clamped to the range of i16"
)]
fn sample(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * 32_767.0) as i16
}

/// The number of samples in `ms` milliseconds.
fn count(ms: u32) -> usize {
    usize::try_from(SAMPLE_RATE * ms / 1_000).unwrap_or(0)
}

/// The time of sample `i`, in seconds.
#[expect(
    clippy::cast_precision_loss,
    reason = "a sound has under 50000 samples"
)]
fn seconds(i: usize) -> f32 {
    i as f32 / SAMPLE_RATE as f32
}

/// Soft bell notes: (frequency in Hz, start and length in ms), each fading
/// with the time constant `fade` (ms).
fn notes(notes: &[(f32, u32, u32)], fade: f32) -> Vec<f32> {
    let end = notes.iter().map(|(_, start, len)| start + len).max();
    let mut wave = vec![0.0; count(end.unwrap_or(0))];
    for &(pitch, start, len) in notes {
        let total = seconds(count(len));
        for (i, out) in wave
            .iter_mut()
            .skip(count(start))
            .take(count(len))
            .enumerate()
        {
            let t = seconds(i);
            let phase = TAU * pitch * t;
            let tone = phase.sin() + 0.25 * (2.0 * phase).sin() + 0.08 * (3.0 * phase).sin();
            // No clicks: 4 ms in, 15 ms out.
            let edges = (t / 0.004).min((total - t) / 0.015).clamp(0.0, 1.0);
            *out += tone * edges * (-t * 1_000.0 / fade).exp();
        }
    }
    wave
}

/// Cloth in the wind: noise between two moving filters, over a low hum.
/// It opens up when `rising`, and is drawn back in otherwise.
fn whoosh(ms: u32, rising: bool) -> Vec<f32> {
    let len = count(ms);
    let total = seconds(len);
    let mut noise = Noise(0x50_48_49_21);
    let (mut high, mut low) = (0.0_f32, 0.0_f32);
    let mut phase = 0.0_f32;
    (0..len)
        .map(|i| {
            let t = seconds(i);
            let p = t / total;
            let open = if rising { p } else { 1.0 - p };
            // The cape beats as it goes.
            let beat = 0.75 + 0.25 * (TAU * 3.2 * t).sin();
            let swell = (p * std::f32::consts::PI).sin().powi(2) * beat;
            let cut = 250.0 + 2_600.0 * open * open;
            high += filter(cut) * (noise.next() - high);
            low += filter(cut * 0.25) * (high - low);
            phase += TAU * (48.0 + 40.0 * open) / seconds_per();
            let hum = phase.sin() * 0.12;
            swell * ((high - low) + hum)
        })
        .collect()
}

#[expect(clippy::cast_precision_loss, reason = "24000 is exact")]
fn seconds_per() -> f32 {
    SAMPLE_RATE as f32
}

/// The coefficient of a one-pole low-pass at `cut` Hz.
fn filter(cut: f32) -> f32 {
    1.0 - (-TAU * cut / seconds_per()).exp()
}

/// White noise, the same every time (xorshift).
struct Noise(u32);

impl Noise {
    #[expect(clippy::cast_precision_loss, reason = "noise")]
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vad::{Earshot, Segment, Segmenter};

    #[test]
    fn they_start_and_end_without_a_click() {
        for sound in Sound::ALL {
            let wave = sound.samples(100);
            let peak = wave.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!((4_000..=16_384).contains(&peak), "{sound:?}: {peak}");
            assert!(wave[0].unsigned_abs() < 400, "{sound:?}");
            assert!(wave[wave.len() - 1].unsigned_abs() < 400, "{sound:?}");
            let jump = wave
                .windows(2)
                .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs());
            assert!(jump.max().unwrap() < 14_000, "{sound:?}");
        }
    }

    #[test]
    fn the_volume_counts() {
        let peak = |volume| {
            let wave = Sound::Reminder.samples(volume);
            wave.iter().map(|s| s.unsigned_abs()).max().unwrap()
        };
        assert_eq!(peak(0), 0);
        assert!(peak(50).abs_diff(peak(100) / 2) < 10);
        assert_eq!(peak(200), peak(100));
    }

    #[test]
    fn they_are_short() {
        for (sound, max) in [
            (Sound::Listen, 250),
            (Sound::Heard, 150),
            (Sound::Ask, 600),
            (Sound::Error, 500),
            (Sound::Reminder, 1_300),
            (Sound::CapeOver, 1_700),
            (Sound::CapeBack, 800),
        ] {
            assert!(sound.samples(50).len() <= count(max), "{sound:?}");
        }
    }

    /// The mic is open while this one plays, and may hear it. The detector
    /// may take it for the start of something, but not for an utterance.
    #[test]
    fn opening_the_mic_is_not_taken_for_speech() {
        for volume in [5, 20, 40, 100] {
            let mut segmenter = Segmenter::new(Earshot::new());
            // 24 kHz to the mic's 16 kHz: two samples for every three.
            let heard: Vec<i16> = Sound::Listen
                .samples(volume)
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|c| [c[0], i16::midpoint(c[1], c[2])])
                .collect();
            let mut segments = segmenter.push(&vec![0; 3_200]);
            segments.extend(segmenter.push(&heard));
            segments.extend(segmenter.push(&vec![0; 16_000]));
            for segment in segments {
                let harmless = match segment {
                    Segment::Started { .. } => true,
                    Segment::Discarded { voiced_ms } => voiced_ms <= 130,
                    _ => false,
                };
                assert!(harmless, "at {volume}: {segment:?}");
            }
        }
    }
}
