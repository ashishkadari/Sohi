//! Speech segmentation (spec §5.4): finds where an utterance starts and
//! ends in the microphone stream.
//!
//! Audio is cut into 16 ms frames (256 samples at 16 kHz). A
//! [`VoiceDetector`] scores each frame from 0 to 1; a frame is voiced at
//! 0.5 or more. [`Segmenter`] turns the scores into events:
//!
//! - **Start:** at least 6 of the last 10 frames (about 100 of 160 ms)
//!   are voiced. Speech is taken to start at the first voiced frame of
//!   that window, or earlier if the frames just before it score 0.35 or
//!   more, or are at least twice as loud as the quietest frame heard so
//!   far. That catches soft onsets such as the "s" of "set" or the "h" of
//!   "how", which the detector does not count as voice (up to 256 ms
//!   back). The utterance keeps 300 ms of audio from before the start.
//! - **End:** 44 unvoiced frames (about 700 ms) in a row.
//! - **Too short:** speech with under 250 ms of voiced frames (a cough, a
//!   click) is dropped and listening goes on.
//! - **Limits:** 8 s without speech ends listening; an utterance is cut
//!   at 30 s.

use std::collections::VecDeque;

/// Samples per frame (16 ms at 16 kHz).
pub const FRAME: usize = 256;
/// Milliseconds per frame.
pub const FRAME_MS: u32 = 16;

const START_WINDOW: u32 = 10;
const START_VOICED: u32 = 6;
/// A frame scoring this much is voiced.
const VOICED: f32 = 0.5;
/// Frames just before speech scoring this much count as its onset.
const ONSET: f32 = 0.35;
/// How far before the first voiced frame an onset may reach.
const ONSET_FRAMES: u32 = 16;
/// A frame this many times louder (RMS) than the noise floor is onset.
const ONSET_LOUDNESS: f32 = 2.0;
/// The noise floor never counts as lower than this (about -66 dBFS), so
/// digital silence does not make every faint sound an onset.
const MIN_FLOOR: f32 = 0.000_5;
const END_SILENT_FRAMES: u32 = 44;
const PREROLL_SAMPLES: usize = 4_800;
/// Audio kept after the last voiced frame, so a soft ending is not cut.
const TAIL_SAMPLES: usize = 2_560;
/// Audio held while waiting: the pre-roll before the earliest possible start.
const WAITING_SAMPLES: usize = PREROLL_SAMPLES + (START_WINDOW + ONSET_FRAMES) as usize * FRAME;
/// Speech with less voice than this is dropped.
pub const MIN_SPEECH_MS: u32 = 250;
/// Listening ends after this long without speech.
pub const NO_SPEECH_MS: u32 = 8_000;
/// An utterance is cut at this length.
pub const MAX_UTTERANCE_MS: u32 = 30_000;

/// Scores frames for voice.
pub trait VoiceDetector {
    /// How likely `frame` (exactly [`FRAME`] samples) is to contain
    /// voice, from 0 to 1.
    fn score(&mut self, frame: &[i16]) -> f32;
}

/// The earshot neural-network detector.
pub struct Earshot {
    detector: Box<earshot::Detector>,
}

impl Earshot {
    /// A fresh detector (about 8 KB of state).
    #[must_use]
    pub fn new() -> Self {
        Self {
            detector: earshot::Detector::default_boxed(),
        }
    }
}

impl Default for Earshot {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceDetector for Earshot {
    fn score(&mut self, frame: &[i16]) -> f32 {
        self.detector.predict_i16(frame)
    }
}

/// A finished utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    /// Pre-roll, speech and a short tail, 16 kHz mono.
    pub samples: Vec<i16>,
    /// When speech started, in ms since listening began.
    pub start_ms: u32,
    /// When the last voiced frame ended, in ms since listening began.
    pub end_ms: u32,
}

impl Utterance {
    /// How long the speech lasted.
    #[must_use]
    pub fn speech_ms(&self) -> u32 {
        self.end_ms - self.start_ms
    }
}

/// Something the segmenter noticed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Speech started at this time (ms since listening began).
    Started {
        /// When.
        at_ms: u32,
    },
    /// Speech with too little voice to be an utterance; listening goes on.
    Discarded {
        /// How much of it was voiced.
        voiced_ms: u32,
    },
    /// The utterance ended. Listening is over.
    Ended(Utterance),
    /// The utterance hit the 30 s cap. Listening is over.
    TooLong(Utterance),
    /// 8 s passed without speech. Listening is over.
    NoSpeech,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Speaking {
        start_frame: u32,
        last_voiced: u32,
        silent_run: u32,
        /// Voiced frames since the start.
        voiced: u32,
    },
    Done,
}

/// Turns a stream of samples into [`Segment`]s.
pub struct Segmenter<V> {
    detector: V,
    phase: Phase,
    /// Frames seen so far.
    frames: u32,
    /// The last [`START_WINDOW`] voiced decisions, newest in bit 0.
    recent: u32,
    /// The last 32 frames that could be an onset, newest in bit 0.
    onset: u32,
    /// The quietest frame RMS heard while waiting.
    floor: f32,
    /// RMS of the newest frame.
    level: f32,
    /// Waiting: the last pre-roll + window of audio. Speaking: the
    /// utterance so far.
    audio: VecDeque<i16>,
    /// Samples not yet making up a whole frame.
    partial: Vec<i16>,
    min_speech_ms: u32,
}

impl<V: VoiceDetector> Segmenter<V> {
    /// A segmenter at the start of listening.
    pub fn new(detector: V) -> Self {
        Self {
            detector,
            phase: Phase::Waiting,
            frames: 0,
            recent: 0,
            onset: 0,
            floor: 1.0,
            level: 0.0,
            audio: VecDeque::with_capacity(WAITING_SAMPLES),
            partial: Vec::with_capacity(FRAME),
            min_speech_ms: MIN_SPEECH_MS,
        }
    }

    /// Keeps utterances with at least `ms` of voice, instead of
    /// [`MIN_SPEECH_MS`].
    #[must_use]
    pub fn with_min_speech_ms(mut self, ms: u32) -> Self {
        self.min_speech_ms = ms;
        self
    }

    /// Whether listening is over (an utterance ended, or no speech came).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Loudness of the newest frame (RMS, 0 to 1), for the level meter.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Milliseconds of audio seen.
    #[must_use]
    pub fn elapsed_ms(&self) -> u32 {
        self.frames * FRAME_MS
    }

    /// Feeds samples of any length. Returns what happened, in order.
    pub fn push(&mut self, mut samples: &[i16]) -> Vec<Segment> {
        let mut events = Vec::new();
        while !samples.is_empty() && !self.is_done() {
            let take = (FRAME - self.partial.len()).min(samples.len());
            self.partial.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.partial.len() == FRAME {
                let frame = std::mem::take(&mut self.partial);
                events.extend(self.push_frame(&frame));
                self.partial = frame;
                self.partial.clear();
            }
        }
        events
    }

    fn push_frame(&mut self, frame: &[i16]) -> Vec<Segment> {
        let score = self.detector.score(frame);
        let voiced = score >= VOICED;
        let index = self.frames;
        self.frames += 1;
        self.recent = ((self.recent << 1) | u32::from(voiced)) & ((1 << START_WINDOW) - 1);
        self.level = rms(frame);
        if self.phase == Phase::Waiting {
            self.floor = self.floor.min(self.level.max(MIN_FLOOR));
        }
        let loud = self.level >= self.floor * ONSET_LOUDNESS;
        self.onset = (self.onset << 1) | u32::from(score >= ONSET || loud);
        self.audio.extend(frame);
        match self.phase {
            Phase::Waiting => self.while_waiting(index),
            Phase::Speaking {
                start_frame,
                last_voiced,
                silent_run,
                voiced: count,
            } => {
                let (last_voiced, silent_run) = if voiced {
                    (index, 0)
                } else {
                    (last_voiced, silent_run + 1)
                };
                self.phase = Phase::Speaking {
                    start_frame,
                    last_voiced,
                    silent_run,
                    voiced: count + u32::from(voiced),
                };
                self.while_speaking(index).into_iter().collect()
            }
            Phase::Done => Vec::new(),
        }
    }

    fn while_waiting(&mut self, index: u32) -> Vec<Segment> {
        if self.recent.count_ones() >= START_VOICED {
            // The oldest voiced frame in the window is where speech began,
            // unless a soft onset runs up to it.
            let first = self.recent.ilog2();
            let mut age = first;
            while age < first + ONSET_FRAMES && (self.onset >> (age + 1)) & 1 == 1 {
                age += 1;
            }
            let start_frame = index - age;
            self.phase = Phase::Speaking {
                start_frame,
                last_voiced: index,
                silent_run: 0,
                voiced: self.recent.count_ones(),
            };
            // Keep the pre-roll before the start and everything after it.
            let keep = PREROLL_SAMPLES + (age as usize + 1) * FRAME;
            let excess = self.audio.len().saturating_sub(keep);
            self.audio.drain(..excess);
            return vec![Segment::Started {
                at_ms: start_frame * FRAME_MS,
            }];
        }
        let excess = self.audio.len().saturating_sub(WAITING_SAMPLES);
        self.audio.drain(..excess);
        if self.elapsed_ms() >= NO_SPEECH_MS {
            self.finish();
            return vec![Segment::NoSpeech];
        }
        Vec::new()
    }

    fn while_speaking(&mut self, index: u32) -> Option<Segment> {
        let Phase::Speaking {
            start_frame,
            last_voiced,
            silent_run,
            voiced,
        } = self.phase
        else {
            return None;
        };
        let start_ms = start_frame * FRAME_MS;
        let end_ms = (last_voiced + 1) * FRAME_MS;
        if (index + 1 - start_frame) * FRAME_MS >= MAX_UTTERANCE_MS {
            let utterance = self.take_utterance(start_ms, end_ms, 0);
            return Some(Segment::TooLong(utterance));
        }
        if silent_run < END_SILENT_FRAMES {
            return None;
        }
        let voiced_ms = voiced * FRAME_MS;
        if voiced_ms < self.min_speech_ms {
            self.phase = Phase::Waiting;
            self.recent = 0;
            self.onset = 0;
            if self.elapsed_ms() >= NO_SPEECH_MS {
                self.finish();
                return Some(Segment::NoSpeech);
            }
            return Some(Segment::Discarded { voiced_ms });
        }
        let trailing = silent_run as usize * FRAME;
        Some(Segment::Ended(
            self.take_utterance(start_ms, end_ms, trailing),
        ))
    }

    /// Ends listening and hands over the audio, keeping only a short tail
    /// of the `trailing` samples of silence.
    fn take_utterance(&mut self, start_ms: u32, end_ms: u32, trailing: usize) -> Utterance {
        let drop_tail = trailing.saturating_sub(TAIL_SAMPLES);
        self.audio
            .truncate(self.audio.len() - drop_tail.min(self.audio.len()));
        let samples = std::mem::take(&mut self.audio).into();
        self.finish();
        Utterance {
            samples,
            start_ms,
            end_ms,
        }
    }

    fn finish(&mut self) {
        self.phase = Phase::Done;
        self.audio = VecDeque::new();
        self.partial = Vec::new();
    }
}

/// The loudness of a frame, 0 (silence) to 1 (full scale), as RMS.
#[must_use]
pub fn rms(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    #[allow(
        clippy::cast_precision_loss,
        reason = "a frame is at most a few thousand samples"
    )]
    let mean = sum / frame.len() as f64;
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the result is between 0 and 1"
    )]
    let level = (mean.sqrt() / 32_768.0) as f32;
    level.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Voiced exactly in the given frame spans (start, end exclusive).
    struct Script(Vec<(u32, u32)>, u32);

    impl VoiceDetector for Script {
        fn score(&mut self, _: &[i16]) -> f32 {
            let index = self.1;
            self.1 += 1;
            if self.0.iter().any(|&(a, b)| (a..b).contains(&index)) {
                1.0
            } else {
                0.0
            }
        }
    }

    /// Scores a soft onset (0.4) before voiced frames 100..150.
    struct Soft(u32, std::ops::Range<u32>);

    impl VoiceDetector for Soft {
        fn score(&mut self, _: &[i16]) -> f32 {
            let index = self.0;
            self.0 += 1;
            match index {
                _ if self.1.contains(&index) => 0.4,
                100..150 => 0.9,
                _ => 0.2,
            }
        }
    }

    #[test]
    fn a_loud_onset_moves_the_start_back() {
        // Quiet room at 100; the speaker breathes in at frame 95 (loud,
        // but not voice); voice from frame 100.
        let mut segmenter = Segmenter::new(Script(vec![(100, 150)], 0));
        let mut events = Vec::new();
        for f in 0..120 {
            let amplitude = if f >= 95 { 5_000 } else { 100 };
            events.extend(segmenter.push(&[amplitude; FRAME]));
            assert!((segmenter.level() - f32::from(amplitude) / 32_768.0).abs() < 1e-6);
        }
        assert_eq!(
            events,
            [Segment::Started {
                at_ms: 95 * FRAME_MS
            }]
        );
    }

    #[test]
    fn a_soft_onset_moves_the_start_back() {
        for (onset, start) in [(97..100, 97), (60..100, 100 - 16), (50..90, 100)] {
            let mut segmenter = Segmenter::new(Soft(0, onset.clone()));
            let mut events = Vec::new();
            for _ in 0..120 {
                events.extend(segmenter.push(&[0; FRAME]));
            }
            assert_eq!(
                events,
                [Segment::Started {
                    at_ms: start * FRAME_MS
                }],
                "onset {onset:?}"
            );
        }
    }

    /// Feeds `frames` frames whose samples carry their frame number, plus
    /// 10 000 so the loudness stays nearly flat (no energy onsets).
    fn run(voiced: &[(u32, u32)], frames: u32) -> Vec<Segment> {
        let mut segmenter = Segmenter::new(Script(voiced.to_vec(), 0));
        let mut events = Vec::new();
        for f in 0..frames {
            let marker = i16::try_from(10_000 + f).unwrap();
            // Odd chunk sizes: frames are assembled across pushes.
            let frame = [marker; FRAME];
            events.extend(segmenter.push(&frame[..100]));
            events.extend(segmenter.push(&frame[100..]));
        }
        events
    }

    fn ended(events: &[Segment]) -> &Utterance {
        events
            .iter()
            .find_map(|e| match e {
                Segment::Ended(u) | Segment::TooLong(u) => Some(u),
                _ => None,
            })
            .expect("an utterance")
    }

    #[test]
    fn speech_starts_at_its_first_voiced_frame() {
        // Voiced from frame 100; the 6th voiced frame (105) triggers.
        let events = run(&[(100, 150)], 300);
        assert_eq!(events[0], Segment::Started { at_ms: 1600 });
        let utterance = ended(&events);
        assert_eq!((utterance.start_ms, utterance.end_ms), (1600, 2400));
    }

    #[test]
    fn scattered_voiced_frames_do_not_start_speech() {
        let voiced: Vec<_> = (0..200).step_by(2).map(|f| (f, f + 1)).collect();
        // 5 of every 10 frames voiced: never 6.
        let events = run(&voiced, 400);
        assert!(!events.iter().any(|e| matches!(e, Segment::Started { .. })));
    }

    #[test]
    fn the_utterance_keeps_300_ms_before_the_start_and_a_short_tail() {
        let events = run(&[(100, 150)], 300);
        let utterance = ended(&events);
        // Pre-roll: frames 82..100 hold 4800 samples (18.75 frames).
        let first = utterance.samples[0];
        assert_eq!(first, 10_081, "starts inside frame 81 (300 ms before 100)");
        assert_eq!(
            utterance.samples.len(),
            PREROLL_SAMPLES + 50 * FRAME + TAIL_SAMPLES
        );
    }

    #[test]
    fn ends_after_700_ms_of_silence() {
        let events = run(&[(100, 150), (160, 170)], 400);
        let utterance = ended(&events);
        // A 10-frame gap does not end it; the end is the last voiced frame.
        assert_eq!(utterance.end_ms, 170 * FRAME_MS);
        assert_eq!(events.len(), 2, "{events:?}");
    }

    #[test]
    fn a_short_burst_is_discarded_and_listening_goes_on() {
        let events = run(&[(50, 60), (200, 240)], 400);
        assert_eq!(events[0], Segment::Started { at_ms: 800 });
        assert_eq!(events[1], Segment::Discarded { voiced_ms: 160 });
        assert_eq!(events[2], Segment::Started { at_ms: 3200 });
        assert_eq!(ended(&events).start_ms, 3200);
    }

    #[test]
    fn a_shorter_minimum_keeps_a_short_word() {
        let script = Script(vec![(50, 60)], 0);
        let mut segmenter = Segmenter::new(script).with_min_speech_ms(100);
        let events: Vec<_> = (0..200).flat_map(|_| segmenter.push(&[0; FRAME])).collect();
        assert_eq!(ended(&events).speech_ms(), 160);
    }

    #[test]
    fn eight_seconds_of_silence_is_no_speech() {
        let events = run(&[], 1000);
        assert_eq!(events, [Segment::NoSpeech]);
        // 8000 ms / 16 ms = frame 500.
        let mut segmenter = Segmenter::new(Script(Vec::new(), 0));
        let mut n = 0;
        while segmenter.push(&[0; FRAME]).is_empty() {
            n += 1;
        }
        assert_eq!((n + 1) * FRAME_MS, NO_SPEECH_MS);
        assert!(segmenter.is_done());
    }

    #[test]
    fn speech_is_cut_at_30_seconds() {
        let events = run(&[(100, 5000)], 3000);
        let Some(Segment::TooLong(utterance)) = events.get(1) else {
            panic!("{:?}", events.get(1));
        };
        assert_eq!(utterance.start_ms, 1600);
        assert_eq!(
            utterance.samples.len(),
            PREROLL_SAMPLES + (MAX_UTTERANCE_MS / FRAME_MS) as usize * FRAME
        );
        assert_eq!(events.len(), 2, "nothing after the cut");
    }

    #[test]
    fn rms_is_zero_to_one() {
        assert!(rms(&[]).abs() < f32::EPSILON);
        assert!(rms(&[0; 256]).abs() < f32::EPSILON);
        assert!((rms(&[i16::MIN; 256]) - 1.0).abs() < 1e-6);
        let half = rms(&[16_384; 256]);
        assert!((half - 0.5).abs() < 1e-4, "{half}");
    }
}
