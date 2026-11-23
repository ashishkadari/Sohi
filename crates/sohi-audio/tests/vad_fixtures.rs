//! Segmentation on real speech (spec §5.4 acceptance): five synthesized
//! phrases, padded with silence and mixed with noise at known offsets.
//! Speech must be found within ±100 ms of where it was placed.

#![allow(clippy::unwrap_used, reason = "test helpers panic on bad fixtures")]

use sohi_audio::vad::{Earshot, Segment, Segmenter, Utterance};
use sohi_audio::wav;

const NAMES: [&str; 5] = ["open_browser", "weather", "timer", "music", "question"];
const TOLERANCE_MS: i64 = 100;

fn fixture(name: &str) -> Vec<i16> {
    let path = format!("{}/tests/fixtures/{name}.wav", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    wav::decode(&bytes).unwrap()
}

fn ms(n: usize) -> usize {
    n * 16
}

/// Deterministic noise in -1..1 (xorshift), so runs repeat exactly.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        #[allow(clippy::cast_precision_loss)]
        let unit = self.0 as f32 / u32::MAX as f32;
        unit * 2.0 - 1.0
    }
}

/// A recording: `lead_ms` of background, the speech scaled by `gain`,
/// then `tail_ms` of background. Background is noise at `noise` (full
/// scale = 1), present throughout.
struct Take {
    samples: Vec<i16>,
    start_ms: i64,
    end_ms: i64,
}

fn take(speech: &[i16], lead_ms: usize, tail_ms: usize, gain: f32, noise: f32) -> Take {
    let lead = ms(lead_ms);
    let mut mix: Vec<f32> = vec![0.0; lead + speech.len() + ms(tail_ms)];
    for (out, &s) in mix[lead..].iter_mut().zip(speech) {
        *out = f32::from(s) * gain;
    }
    let mut rng = Noise(0x5EED_1234);
    for out in &mut mix {
        *out += rng.next() * noise * 32_767.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let samples = mix
        .iter()
        .map(|&x| x.clamp(-32_768.0, 32_767.0) as i16)
        .collect();
    let to_ms = |n: usize| i64::try_from(n / 16).unwrap();
    Take {
        samples,
        start_ms: to_ms(lead),
        end_ms: to_ms(lead + speech.len()),
    }
}

/// Runs the take through a fresh segmenter in 10 ms chunks, the way
/// WASAPI hands audio over.
fn segment(samples: &[i16]) -> Vec<Segment> {
    let mut segmenter = Segmenter::new(Earshot::new());
    let mut events = Vec::new();
    for chunk in samples.chunks(160) {
        events.extend(segmenter.push(chunk));
        if segmenter.is_done() {
            break;
        }
    }
    events
}

fn utterance(events: &[Segment]) -> Option<&Utterance> {
    events.iter().find_map(|e| match e {
        Segment::Ended(u) => Some(u),
        _ => None,
    })
}

fn check(name: &str, take: &Take) {
    let events = segment(&take.samples);
    let found = utterance(&events).unwrap_or_else(|| panic!("{name}: no utterance in {events:?}"));
    let start = i64::from(found.start_ms);
    let end = i64::from(found.end_ms);
    println!(
        "{name}: start {start} (want {}), end {end} (want {})",
        take.start_ms, take.end_ms
    );
    assert!(
        (start - take.start_ms).abs() <= TOLERANCE_MS,
        "{name}: started at {start} ms, speech starts at {} ms",
        take.start_ms
    );
    assert!(
        (end - take.end_ms).abs() <= TOLERANCE_MS,
        "{name}: ended at {end} ms, speech ends at {} ms",
        take.end_ms
    );
}

#[test]
fn finds_speech_in_quiet_rooms() {
    for name in NAMES {
        check(name, &take(&fixture(name), 1_200, 1_500, 1.0, 0.001));
    }
}

#[test]
fn finds_speech_in_noise() {
    // Speech RMS is about 0.1 full scale; noise at 0.01 RMS (uniform
    // noise of amplitude a has RMS a/√3) is about 20 dB below it.
    for name in NAMES {
        check(name, &take(&fixture(name), 800, 1_500, 1.0, 0.017));
    }
}

#[test]
fn finds_quiet_speech() {
    for name in NAMES {
        check(name, &take(&fixture(name), 1_000, 1_500, 0.15, 0.0005));
    }
}

#[test]
fn ignores_a_short_burst_before_speech() {
    let speech = fixture("weather");
    // 200 ms of speech, a pause, then the full phrase.
    let burst = take(&speech[..ms(200)], 500, 1_200, 1.0, 0.0);
    let main = take(&speech, 0, 1_500, 1.0, 0.0);
    let mut samples = burst.samples.clone();
    samples.extend_from_slice(&main.samples);
    let offset = i64::try_from(burst.samples.len() / 16).unwrap();
    let events = segment(&samples);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Segment::Discarded { .. })),
        "{events:?}"
    );
    check(
        "after burst",
        &Take {
            samples,
            start_ms: offset,
            end_ms: offset + main.end_ms,
        },
    );
}

#[test]
fn silence_times_out() {
    let quiet = take(&[], 9_000, 0, 1.0, 0.002);
    let events = segment(&quiet.samples);
    assert_eq!(events, [Segment::NoSpeech]);
}
