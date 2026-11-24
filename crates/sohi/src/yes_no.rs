//! Hearing a yes or a no during a voice turn (spec §5.6). The answer is
//! transcribed and matched against a fixed list; no model decides.

use std::time::{Duration, Instant};

use sohi_audio::ring::{RING_SAMPLES, capture_ring};
use sohi_audio::vad::{Earshot, Segment, Segmenter, Utterance};
use sohi_audio::wav;
use sohi_core::answer::parse_answer;
use sohi_core::config::SttConfig;
use sohi_providers::HttpClient;

use crate::actions::Ear;
use crate::speak::Speech;
use crate::voice::{Source, TICK, drain, transcribe};

/// The mic is open this long for an answer to begin.
const WINDOW: Duration = Duration::from_secs(4);
/// "No" is shorter than anything else worth hearing.
const MIN_SPEECH_MS: u32 = 100;
/// What Sohi said last is still on its way out of the speakers.
const SETTLE: Duration = Duration::from_millis(150);

/// The ear of a voice turn.
pub struct VoiceEar<'a> {
    /// The turn.
    pub turn: u64,
    /// For transcribing the answer.
    pub http: &'a HttpClient,
    /// `[stt]`.
    pub stt: &'a SttConfig,
    /// What Sohi is saying.
    pub speech: &'a Speech,
    /// Says what is written so far, finished sentence or not.
    pub flush: &'a dyn Fn(),
}

impl Ear for VoiceEar<'_> {
    /// The mic is never open while Sohi speaks.
    async fn ready(&self) {
        (self.flush)();
        let mut spoke = false;
        while !self.speech.is_quiet() {
            spoke = true;
            tokio::time::sleep(TICK).await;
        }
        if spoke {
            tokio::time::sleep(SETTLE).await;
        }
        self.speech.will_resume();
    }

    async fn listen(&self) -> Option<bool> {
        let utterance = self.hear().await?;
        let ended = Instant::now();
        let audio = wav::encode(&utterance.samples);
        let text = match transcribe(self.http, self.stt, "", &audio).await {
            Ok(text) => text,
            Err(error) => {
                log::warn!("turn {}: answer not transcribed: {error}", self.turn);
                return None;
            }
        };
        let answer = parse_answer(&text);
        log::info!(
            "turn {}: {} ms answer transcribed in {} ms: {}",
            self.turn,
            utterance.speech_ms(),
            ended.elapsed().as_millis(),
            match answer {
                Some(true) => "yes",
                Some(false) => "no",
                None => "neither yes nor no",
            }
        );
        answer
    }
}

impl VoiceEar<'_> {
    /// Opens the mic until something was said, or nothing for [`WINDOW`].
    async fn hear(&self) -> Option<Utterance> {
        let turn = self.turn;
        let (writer, mut reader) = capture_ring();
        let opened = Instant::now();
        let source = Source::open(writer, opened, "SOHI_DEV_INJECT_ANSWER")
            .inspect_err(|err| log::warn!("turn {turn}: cannot open the mic: {err}"))
            .ok()?;
        let mut segmenter = Segmenter::new(Earshot::new()).with_min_speech_ms(MIN_SPEECH_MS);
        let mut samples = vec![0i16; RING_SAMPLES];
        let mut started = false;
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let heard = loop {
            tick.tick().await;
            let mut heard = None;
            for segment in drain(&mut reader, &mut segmenter, &mut samples) {
                match segment {
                    Segment::Started { .. } => started = true,
                    Segment::Discarded { .. } => started = false,
                    Segment::Ended(u) | Segment::TooLong(u) => heard = Some(u),
                    Segment::NoSpeech => {}
                }
            }
            let over = !started && opened.elapsed() >= WINDOW;
            if heard.is_some() || over || source.has_failed() {
                break heard;
            }
        };
        source.close(turn, opened);
        heard
    }
}
