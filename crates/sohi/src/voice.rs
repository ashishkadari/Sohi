//! Voice turns on the runtime thread (spec §3.3, §5.4, §5.5).
//!
//! The mic opens when the turn starts, and the connection to Mistral is
//! warmed at the same time. Every 32 ms the samples captured so far go
//! through the segmenter and the level goes to the UI (about 30 Hz). When
//! the utterance ends the mic closes, the audio is transcribed, and the
//! transcript is answered aloud (see [`crate::speak`]).
//!
//! Dropping the future (cancel, barge-in) drops the capture, which stops
//! the mic thread.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use sohi_audio::ring::{CaptureReader, CaptureWriter, capture_ring};
use sohi_audio::vad::{Earshot, Segment, Segmenter, Utterance, rms};
use sohi_audio::wav;
use sohi_core::chat::Message;
use sohi_core::config::SttConfig;
use sohi_core::persona::{HEARD_NAME, correct_heard_name};
use sohi_core::secret::KeyName;
use sohi_providers::voxtral::TranscribeRequest;
use sohi_providers::{HttpClient, Mistral, ProviderError, Voxtral, mistral};

use crate::chat::{ListenTurn, ProviderRunner, Update};
use crate::reply::read_key;
use crate::runtime::UiLink;
use crate::speak::{Aloud, answer_aloud};

/// How often captured audio is processed (and the level sent).
pub const TICK: Duration = Duration::from_millis(32);
/// How often a mic that was lost while listening is replaced.
const MAX_REOPENS: u32 = 2;
/// Windows needs a moment to choose another device.
const REOPEN_AFTER: Duration = Duration::from_millis(400);
/// A warmed connection counts as warm this long (spec §3.3).
const PREWARM_FRESH: Duration = Duration::from_secs(60);
/// The prewarm request gets this long.
const PREWARM_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs one voice turn to completion, sending its updates.
pub async fn run(runner: &mut ProviderRunner, listen: ListenTurn, ui: &UiLink<Update>) {
    let turn = listen.chat.turn;
    let http = match runner.client() {
        Ok(http) => http,
        Err(error) => {
            ui.send(Update::Failed { turn, error });
            return;
        }
    };
    prewarm(runner, &http);
    let Some(utterance) = listen_for_speech(turn, ui).await else {
        return;
    };
    let ended = Instant::now();
    let audio = wav::encode(&utterance.samples);
    #[cfg(feature = "dev-record")]
    record(turn, &audio);
    let text = match transcribe(&http, &listen.stt, HEARD_NAME, &audio).await {
        Ok(text) => correct_heard_name(&text),
        Err(error) => {
            log::warn!("turn {turn}: transcription failed: {error}");
            ui.send(Update::Failed { turn, error });
            return;
        }
    };
    log::info!(
        "turn {turn}: {} ms of speech ({} KB) at {:.0} dBFS transcribed in {} ms ({} chars)",
        utterance.speech_ms(),
        audio.len() / 1024,
        20.0 * rms(&utterance.samples).max(1e-6).log10(),
        ended.elapsed().as_millis(),
        text.chars().count()
    );
    ui.send(Update::Transcript {
        turn,
        text: text.clone(),
    });
    if text.is_empty() {
        return;
    }
    let mut chat = listen.chat;
    chat.messages.push(Message::user(text));
    let aloud = Aloud {
        http: &http,
        turn: &chat,
        stt: &listen.stt,
        tts: &listen.tts,
        heard_at: ended,
        toolbox: &runner.toolbox,
    };
    answer_aloud(aloud, ui).await;
}

/// Warms the connection (TCP + TLS) in the background, at most once a
/// minute, so the upload does not pay for the handshake.
fn prewarm(runner: &mut ProviderRunner, http: &HttpClient) {
    if runner
        .prewarmed
        .is_some_and(|at| at.elapsed() < PREWARM_FRESH)
    {
        return;
    }
    runner.prewarmed = Some(Instant::now());
    let mistral = Mistral::new(http.clone(), mistral::BASE_URL);
    tokio::spawn(async move {
        let started = Instant::now();
        let Ok(key) = read_key(KeyName::Mistral) else {
            return;
        };
        match mistral.models(&key, PREWARM_TIMEOUT).await {
            Ok(_) => log::info!("prewarm done in {} ms", started.elapsed().as_millis()),
            Err(error) => log::info!("prewarm failed: {error}"),
        }
    });
}

/// Opens the mic. Returns `None` (having told the UI) if that fails.
fn open_mic(turn: u64, ui: &UiLink<Update>) -> Option<(Source, CaptureReader, Instant)> {
    let (writer, reader) = capture_ring();
    let opened = Instant::now();
    match Source::open(writer, opened, "SOHI_DEV_INJECT") {
        Ok(source) => Some((source, reader, opened)),
        Err(err) => {
            log::warn!("turn {turn}: cannot open the mic: {err}");
            ui.send(Update::MicFailed { turn });
            None
        }
    }
}

/// Opens the mic and waits for an utterance. Returns `None` (having told
/// the UI why) if nobody spoke or the mic failed.
async fn listen_for_speech(turn: u64, ui: &UiLink<Update>) -> Option<Utterance> {
    let (mut source, mut reader, mut opened) = open_mic(turn, ui)?;
    let mut reopened = 0;
    let mut segmenter = Segmenter::new(Earshot::new());
    let mut samples = vec![0i16; sohi_audio::ring::RING_SAMPLES];
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let events = drain(&mut reader, &mut segmenter, &mut samples);
        ui.send(Update::Level {
            turn,
            level: segmenter.level(),
        });
        for event in events {
            if let ControlFlow::Break(done) = on_segment(turn, event, ui) {
                source.close(turn, opened);
                return done;
            }
        }
        if source.has_failed() {
            let worked = source.has_delivered();
            source.close(turn, opened);
            log::warn!("turn {turn}: the mic stopped delivering audio");
            // Probably unplugged: go on with whatever is the default now.
            // What was said so far is kept.
            reopened += 1;
            if !worked || reopened > MAX_REOPENS {
                ui.send(Update::MicFailed { turn });
                return None;
            }
            tokio::time::sleep(REOPEN_AFTER).await;
            (source, reader, opened) = open_mic(turn, ui)?;
        }
    }
}

/// Feeds everything captured so far to the segmenter.
pub fn drain(
    reader: &mut CaptureReader,
    segmenter: &mut Segmenter<Earshot>,
    buffer: &mut [i16],
) -> Vec<Segment> {
    let dropped = reader.take_dropped();
    if dropped > 0 {
        log::warn!("capture ring overflowed: {dropped} samples dropped");
    }
    let mut events = Vec::new();
    loop {
        let n = reader.read(buffer);
        if n == 0 {
            return events;
        }
        events.extend(segmenter.push(&buffer[..n]));
    }
}

/// Reports a segment. `Break` ends listening: with the utterance, or
/// with nothing if nobody spoke.
fn on_segment(turn: u64, event: Segment, ui: &UiLink<Update>) -> ControlFlow<Option<Utterance>> {
    match event {
        Segment::Started { at_ms } => {
            log::info!("turn {turn}: speech started at {at_ms} ms");
            ui.send(Update::SpeechStarted { turn });
            ControlFlow::Continue(())
        }
        Segment::Discarded { voiced_ms } => {
            log::info!("turn {turn}: ignored {voiced_ms} ms of sound");
            ControlFlow::Continue(())
        }
        Segment::Ended(utterance) | Segment::TooLong(utterance) => {
            log::info!(
                "turn {turn}: speech {}-{} ms",
                utterance.start_ms,
                utterance.end_ms
            );
            ui.send(Update::SpeechEnded { turn });
            ControlFlow::Break(Some(utterance))
        }
        Segment::NoSpeech => {
            log::info!("turn {turn}: no speech");
            ui.send(Update::NoSpeech { turn });
            ControlFlow::Break(None)
        }
    }
}

/// `dev-record`: saves the utterance for tuning transcription.
#[cfg(feature = "dev-record")]
fn record(turn: u64, audio: &[u8]) {
    let Some(dir) = std::env::var_os("SOHI_DEV_RECORD") else {
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("turn{turn:02}.wav"));
    if let Err(err) = std::fs::write(&path, audio) {
        log::warn!("dev-record: {}: {err}", path.display());
    }
}

pub async fn transcribe(
    http: &HttpClient,
    stt: &SttConfig,
    bias: &str,
    audio: &[u8],
) -> Result<String, ProviderError> {
    let key = read_key(KeyName::Mistral)?;
    let request = TranscribeRequest {
        model: &stt.model,
        language: &stt.language,
        bias,
        wav: audio,
    };
    let transcript = Voxtral::public(http.clone())
        .transcribe(&key, &request)
        .await?;
    Ok(transcript.text)
}

/// Where the audio comes from: the mic, or (with the `dev-inject` feature
/// and `SOHI_DEV_INJECT` set to a WAV file) that file played in real time.
pub enum Source {
    Mic(sohi_win::Capture),
    #[cfg(feature = "dev-inject")]
    Inject(tokio::task::JoinHandle<()>),
}

impl Source {
    /// Starts capturing. `inject` is the variable naming the WAV file.
    pub fn open(
        writer: CaptureWriter,
        opened: Instant,
        #[cfg_attr(not(feature = "dev-inject"), allow(unused_variables))] inject: &str,
    ) -> Result<Self, String> {
        #[cfg(feature = "dev-inject")]
        if let Some(path) = std::env::var_os(inject) {
            return inject::start(writer, &path).map(Self::Inject);
        }
        sohi_win::Capture::open(writer, opened)
            .map(Self::Mic)
            .map_err(|e| e.to_string())
    }

    /// Whether the mic stopped by itself.
    pub fn has_failed(&self) -> bool {
        match self {
            Self::Mic(capture) => capture.is_finished(),
            #[cfg(feature = "dev-inject")]
            Self::Inject(_) => false,
        }
    }

    /// Whether the mic delivered any audio.
    pub fn has_delivered(&self) -> bool {
        match self {
            Self::Mic(capture) => capture.first_packet_micros().is_some(),
            #[cfg(feature = "dev-inject")]
            Self::Inject(_) => true,
        }
    }

    /// Stops capturing and logs how quickly the mic delivered audio.
    pub fn close(self, turn: u64, opened: Instant) {
        match self {
            Self::Mic(capture) => {
                if let Some(first) = capture.first_packet_micros() {
                    log::info!(
                        "turn {turn}: first mic packet {} ms after open",
                        first / 1000
                    );
                }
                if let Err(err) = capture.finish() {
                    log::warn!("turn {turn}: the mic did not close cleanly: {err}");
                }
            }
            #[cfg(feature = "dev-inject")]
            Self::Inject(task) => task.abort(),
        }
        log::info!(
            "turn {turn}: mic closed after {} ms",
            opened.elapsed().as_millis()
        );
    }
}

/// `dev-inject`: a WAV file stands in for the mic, for repeatable tests
/// (spec §10.6 P4).
#[cfg(feature = "dev-inject")]
mod inject {
    use std::ffi::OsStr;
    use std::time::Duration;

    use sohi_audio::ring::CaptureWriter;

    /// 10 ms blocks, like WASAPI.
    const BLOCK: usize = 160;
    /// Silence before the file, as if the user took a moment to start.
    const LEAD_BLOCKS: usize = 50;

    /// Plays `path` into `writer` in real time, then silence until
    /// stopped.
    pub fn start(
        writer: CaptureWriter,
        path: &OsStr,
    ) -> Result<tokio::task::JoinHandle<()>, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let samples = sohi_audio::wav::decode(&bytes).map_err(|e| e.to_string())?;
        log::info!(
            "dev-inject: playing {} ({} samples)",
            path.display(),
            samples.len()
        );
        Ok(tokio::spawn(async move {
            let mut writer = writer;
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            let silence = [0i16; BLOCK];
            let blocks = std::iter::repeat_n(&silence[..], LEAD_BLOCKS)
                .chain(samples.chunks(BLOCK))
                .chain(std::iter::repeat(&silence[..]));
            for block in blocks {
                tick.tick().await;
                if writer.is_closed() {
                    return;
                }
                writer.push(block);
            }
        }))
    }
}

#[cfg(all(test, feature = "dev-inject"))]
mod tests {
    use sohi_core::config::{LlmConfig, SttConfig, TtsConfig};
    use sohi_core::persona::{Channel, system_prompt};
    use sohi_win::{AppWindow, EventQueue};

    use super::*;
    use crate::chat::ChatTurn;

    /// The stages of a spoken turn, in order.
    const STAGES: [&str; 7] = [
        "speech started",
        "speech ended",
        "transcript",
        "delta",
        "speaking",
        "done",
        "played",
    ];

    fn stage(update: &Update) -> Option<&'static str> {
        Some(match update {
            Update::SpeechStarted { .. } => "speech started",
            Update::SpeechEnded { .. } => "speech ended",
            Update::Transcript { .. } => "transcript",
            Update::Delta { .. } => "delta",
            Update::SpeakingStarted { .. } => "speaking",
            Update::Done { .. } => "done",
            Update::PlaybackDone { .. } => "played",
            Update::Level { .. } => return None,
            other => panic!("unexpected {other:?}"),
        })
    }

    /// Whole voice turns against the live services (spec §10.6 P5): the
    /// injected question is heard, answered and spoken, in that order, and
    /// the time from the end of the utterance to the first audio is within
    /// budget (p50 1.5 s, p95 2.5 s). Run with:
    /// `SOHI_DEV_INJECT=<question.wav> SOHI_DEV_QUIET=1 cargo test -p sohi
    /// --features dev-inject -- --ignored end_to_end --nocapture`
    #[tokio::test]
    #[ignore = "needs the internet, the stored Mistral key and SOHI_DEV_INJECT"]
    async fn end_to_end() {
        assert!(std::env::var_os("SOHI_DEV_INJECT").is_some());
        let turns = std::env::var("SOHI_E2E_TURNS").map_or(20, |n| n.parse().unwrap());
        let window = AppWindow::create("Sohi.Test.Voice", 0xC141, EventQueue::new()).unwrap();
        let mut runner = ProviderRunner::default();
        let mut waits = Vec::new();
        for turn in 1..=turns {
            let (ui, updates) = crate::runtime::test_link(window.remote());
            let chat = ChatTurn {
                turn,
                model: LlmConfig::default().model,
                fallback_model: String::new(),
                offline: None,
                system: system_prompt(Channel::Spoken, "", &sohi_win::local_time(), true),
                messages: Vec::new(),
                policy: sohi_tools::Policy::default(),
            };
            let listen = ListenTurn {
                chat,
                stt: SttConfig::default(),
                tts: TtsConfig::default(),
            };
            let seen = async {
                let mut seen: Vec<(&str, Instant)> = Vec::new();
                while seen.last().is_none_or(|(s, _)| *s != "played") {
                    for name in updates.try_iter().filter_map(|u| stage(&u)) {
                        if seen.iter().all(|(s, _)| *s != name) {
                            seen.push((name, Instant::now()));
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                seen
            };
            let ((), seen) = tokio::join!(run(&mut runner, listen, &ui), seen);
            let at = |name: &str| seen.iter().find(|(s, _)| *s == name).unwrap().1;
            for pair in STAGES.windows(2) {
                // Speaking may start before or after the text is complete.
                let either = pair.contains(&"done");
                assert!(either || at(pair[0]) <= at(pair[1]), "{seen:?}");
            }
            waits.push((at("speaking") - at("speech ended")).as_millis());
        }
        waits.sort_unstable();
        let p50 = waits[waits.len() / 2];
        let p95 = waits[(waits.len() * 95).div_ceil(100) - 1];
        println!("{turns} turns: first audio p50 {p50} ms, p95 {p95} ms ({waits:?})");
        assert!(p50 <= 1_500 && p95 <= 2_500);
    }
}
