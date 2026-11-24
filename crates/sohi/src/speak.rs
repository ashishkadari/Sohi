//! Speaking a voice turn's reply (spec §5.5).
//!
//! Three things run side by side on the runtime thread: the model writes
//! the reply, each finished sentence is synthesized, and the speech so far
//! is fed to the render thread. Speech that cannot be had online comes
//! from the offline voice instead, and if that fails too the turn carries
//! on silently: the reply is in the pill either way.
//!
//! Dropping the future (cancel, barge-in) drops the [`Playback`], which
//! silences the device. The mic was closed before this starts, so it is
//! never open while Sohi speaks (half-duplex).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use sohi_audio::playback::{PlaybackMeter, PlaybackWriter, playback_ring};
use sohi_core::config::{SttConfig, TtsConfig};
use sohi_core::secret::KeyName;
use sohi_core::speech::SentenceChunker;
use sohi_providers::{
    HttpClient, MistralTts, ProviderError, SpeechAudio, TtsProvider, mistral_tts,
};
use sohi_win::Playback;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::actions::{Actions, Toolbox};
use crate::chat::{ChatTurn, Update};
use crate::reply::{chat, read_key};
use crate::runtime::UiLink;
use crate::yes_no::VoiceEar;

/// How often speech is fed to the render thread and its state checked.
const TICK: Duration = Duration::from_millis(16);
/// How often a device that was lost while it played is replaced in a turn.
const MAX_REOPENS: u32 = 4;
/// Windows needs a moment to choose another device.
const REOPEN_AFTER: Duration = Duration::from_millis(400);
/// Synthesis pauses while this much speech is waiting to be played.
const AHEAD: Duration = Duration::from_secs(15);

/// A voice turn to answer.
pub struct Aloud<'a> {
    /// For the models and the speech.
    pub http: &'a HttpClient,
    /// The conversation, ending with what the user said.
    pub turn: &'a ChatTurn,
    /// `[stt]`, for spoken answers to questions.
    pub stt: &'a SttConfig,
    /// `[tts]`.
    pub tts: &'a TtsConfig,
    /// When the user stopped speaking, for the latency log.
    pub heard_at: Instant,
    /// What tools run on.
    pub toolbox: &'a Toolbox,
}

/// Answers a turn in the pill and aloud, and reports how it ended.
pub async fn answer_aloud(aloud: Aloud<'_>, ui: &UiLink<Update>) {
    let Aloud { http, turn, .. } = aloud;
    let id = turn.turn;
    let (sentences_tx, sentences) = unbounded_channel();
    let chunker = RefCell::new(SentenceChunker::new());
    let speech = Speech::default();
    let say = |sentences: Vec<String>| {
        for sentence in sentences {
            speech.unsaid.update(|n| n + 1);
            let _ = sentences_tx.send(sentence);
        }
    };
    let flush = || say(chunker.borrow_mut().flush());
    let write = async {
        let on_text = |delta: &str| {
            ui.send(Update::Delta {
                turn: id,
                text: delta.to_owned(),
            });
            say(chunker.borrow_mut().push(delta));
        };
        let ear = VoiceEar {
            turn: id,
            http,
            stt: aloud.stt,
            speech: &speech,
            flush: &flush,
        };
        let mut actions = Actions::new(id, &turn.policy, aloud.toolbox, ui, ear);
        let reply = chat(http, turn, &mut actions, &on_text).await?;
        flush();
        let runs = actions.take_runs();
        ui.send(Update::Done {
            turn: id,
            reply,
            runs,
        });
        // Nothing more to say: lets the synthesis end.
        let _ = sentences_tx.send(String::new());
        Ok::<(), ProviderError>(())
    };
    let voice = Voice::new(http, aloud.tts);
    let heard = async {
        speak(id, voice, sentences, aloud.heard_at, &speech, ui).await;
        Ok(())
    };
    match tokio::try_join!(write, heard) {
        Ok(_) => {
            ui.send(Update::PlaybackDone { turn: id });
        }
        Err(error) => {
            log::warn!("turn {id} failed: {error}");
            ui.send(Update::Failed { turn: id, error });
        }
    }
}

/// What the speaking side knows about what is being said.
#[derive(Default)]
pub struct Speech {
    queue: Queue,
    /// Sentences that are not synthesized yet.
    unsaid: Cell<usize>,
    /// Samples wait in the device's ring.
    pending: Cell<bool>,
    /// Nothing more will be queued.
    complete: Cell<bool>,
    /// The UI must be told again when speech goes on.
    resume: Cell<bool>,
}

impl Speech {
    /// Whether everything written so far was said.
    pub fn is_quiet(&self) -> bool {
        self.unsaid.get() == 0 && self.queue.borrow().is_empty() && !self.pending.get()
    }

    /// Reports the next speech as [`Update::SpeakingStarted`] again.
    pub fn will_resume(&self) {
        self.resume.set(true);
    }
}

/// Synthesizes the sentences and plays them until all was heard.
async fn speak(
    turn: u64,
    voice: Voice,
    sentences: UnboundedReceiver<String>,
    heard_at: Instant,
    speech: &Speech,
    ui: &UiLink<Update>,
) {
    let mut speaker = Speaker::new(turn, heard_at, speech, ui);
    if let Some(rate) = voice.expected_rate() {
        // Opening the device takes about 300 ms: do it while the model
        // thinks.
        speaker.open(rate);
    }
    let synthesize = async {
        synthesize(turn, voice, sentences, speech).await;
        speech.complete.set(true);
    };
    tokio::join!(synthesize, speaker.run());
}

/// Speech waiting to be played.
struct Piece {
    rate: u32,
    samples: Vec<i16>,
    /// How many were played.
    used: usize,
}

type Queue = RefCell<VecDeque<Piece>>;

/// How much speech is waiting.
fn waiting(queue: &Queue) -> Duration {
    let queue = queue.borrow();
    queue
        .iter()
        .map(|p| {
            let left = u64::try_from(p.samples.len() - p.used).unwrap_or(u64::MAX);
            Duration::from_micros(left * 1_000_000 / u64::from(p.rate.max(1)))
        })
        .sum()
}

/// Turns sentences into speech until there are no more.
async fn synthesize(
    turn: u64,
    mut voice: Voice,
    mut sentences: UnboundedReceiver<String>,
    speech: &Speech,
) {
    let queue = &speech.queue;
    while let Some(sentence) = sentences.recv().await {
        // Nothing more to come.
        if sentence.is_empty() {
            return;
        }
        while waiting(queue) > AHEAD {
            tokio::time::sleep(TICK).await;
        }
        let mut began = None;
        let asked = Instant::now();
        let said = voice.say(&sentence, queue, &mut began).await;
        if let Some(at) = began {
            log::info!(
                "turn {turn}: {} chars to speak, first samples after {} ms, all after {} ms",
                sentence.chars().count(),
                at.duration_since(asked).as_millis(),
                asked.elapsed().as_millis()
            );
        }
        speech.unsaid.update(|n| n.saturating_sub(1));
        let Err(error) = said else {
            continue;
        };
        if !matches!(voice, Voice::Online(_)) {
            log::warn!("turn {turn}: cannot speak: {error}");
            continue;
        }
        log::warn!("turn {turn}: online speech failed ({error}); using the offline voice");
        voice = voice.offline();
        // A sentence already under way is not started over.
        if began.is_none()
            && let Err(error) = voice.say(&sentence, queue, &mut began).await
        {
            log::warn!("turn {turn}: cannot speak: {error}");
        }
    }
}

/// The samples to play: as they are, or (with the `dev-inject` feature
/// and `SOHI_DEV_QUIET` set) at -48 dB, so repeated tests stay unheard.
fn audible(samples: &[i16]) -> Vec<i16> {
    #[cfg(feature = "dev-inject")]
    if std::env::var_os("SOHI_DEV_QUIET").is_some() {
        return samples.iter().map(|s| s / 256).collect();
    }
    samples.to_vec()
}

/// Where speech comes from.
enum Voice {
    Online(Box<OnlineVoice>),
    Offline(OfflineVoice),
}

struct OnlineVoice {
    tts: MistralTts,
    /// What to use if it fails.
    fallback: OfflineVoice,
}

impl Voice {
    /// The configured voice; the offline one if there is no key.
    fn new(http: &HttpClient, config: &TtsConfig) -> Self {
        let fallback = OfflineVoice {
            name: config.offline_voice.clone(),
        };
        if config.provider.trim() != "mistral" {
            return Self::Offline(fallback);
        }
        match read_key(KeyName::Mistral) {
            Ok(key) => Self::Online(Box::new(OnlineVoice {
                tts: MistralTts::public(http.clone(), key, &config.voice),
                fallback,
            })),
            Err(error) => {
                log::info!("online speech is not set up: {error}");
                Self::Offline(fallback)
            }
        }
    }

    /// The sample rate, if it is known in advance.
    fn expected_rate(&self) -> Option<u32> {
        match self {
            Self::Online(_) => Some(mistral_tts::SAMPLE_RATE),
            Self::Offline(_) => None,
        }
    }

    /// The offline voice, from now on.
    fn offline(self) -> Self {
        match self {
            Self::Online(online) => Self::Offline(online.fallback),
            Self::Offline(voice) => Self::Offline(voice),
        }
    }

    /// Synthesizes one sentence into the queue. `began` is when the
    /// first of it arrived.
    async fn say(
        &self,
        sentence: &str,
        queue: &Queue,
        began: &mut Option<Instant>,
    ) -> Result<(), ProviderError> {
        let mut on_audio = |audio: SpeechAudio<'_>| {
            began.get_or_insert_with(Instant::now);
            queue.borrow_mut().push_back(Piece {
                rate: audio.sample_rate,
                samples: audible(audio.samples),
                used: 0,
            });
        };
        match self {
            Self::Online(voice) => voice.tts.speak(sentence, &mut on_audio).await,
            Self::Offline(voice) => voice.speak(sentence, &mut on_audio).await,
        }
    }
}

/// The Windows `OneCore` voice.
struct OfflineVoice {
    name: String,
}

impl TtsProvider for OfflineVoice {
    async fn speak(
        &self,
        text: &str,
        on_audio: &mut dyn FnMut(SpeechAudio<'_>),
    ) -> Result<(), ProviderError> {
        let (text, name) = (text.to_owned(), self.name.clone());
        let rendered =
            tokio::task::spawn_blocking(move || sohi_win::synthesize_offline(&text, &name))
                .await
                .map_err(|e| ProviderError::BadResponse(format!("offline speech: {e}")))?;
        let (sample_rate, samples) =
            rendered.map_err(|e| ProviderError::BadResponse(format!("offline speech: {e}")))?;
        on_audio(SpeechAudio {
            sample_rate,
            samples: &samples,
        });
        Ok(())
    }
}

/// Plays the queue.
struct Speaker<'a> {
    turn: u64,
    heard_at: Instant,
    speech: &'a Speech,
    ui: &'a UiLink<Update>,
    output: Option<Output>,
    /// Whether speech was heard in this turn.
    speaking: bool,
    /// How often a lost device was replaced in this turn.
    reopened: u32,
    /// When the next device may be opened.
    open_at: Option<Instant>,
}

/// An open output device.
struct Output {
    rate: u32,
    playback: Playback,
    /// `None` once its last sample was written.
    writer: Option<PlaybackWriter>,
    meter: PlaybackMeter,
}

impl<'a> Speaker<'a> {
    fn new(turn: u64, heard_at: Instant, speech: &'a Speech, ui: &'a UiLink<Update>) -> Self {
        Self {
            turn,
            heard_at,
            speech,
            ui,
            output: None,
            speaking: false,
            reopened: 0,
            open_at: None,
        }
    }

    fn open(&mut self, rate: u32) {
        let (writer, reader) = playback_ring(rate);
        let meter = writer.meter();
        match Playback::open(reader, self.heard_at) {
            Ok(playback) => {
                self.output = Some(Output {
                    rate,
                    playback,
                    writer: Some(writer),
                    meter,
                });
            }
            Err(err) => log::warn!("turn {}: cannot play speech: {err}", self.turn),
        }
    }

    /// Feeds the device until everything was heard, or it failed.
    async fn run(&mut self) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.report();
            let done = self.feed();
            self.speech.pending.set(self.is_pending());
            if done {
                break;
            }
        }
        self.speech.pending.set(false);
        // A silent device cannot end while speech is still being made.
        while !self.speech.complete.get() {
            self.speech.queue.borrow_mut().clear();
            tick.tick().await;
        }
    }

    fn is_pending(&self) -> bool {
        let writer = self.output.as_ref().and_then(|o| o.writer.as_ref());
        writer.is_some_and(PlaybackWriter::is_pending)
    }

    /// Tells the UI the level, and that speech started.
    fn report(&mut self) {
        let Some(output) = &self.output else {
            return;
        };
        let Some(first) = output.playback.first_audio_micros() else {
            return;
        };
        if !self.speaking {
            self.speaking = true;
            log::info!(
                "turn {}: first audio {} ms after the utterance ended",
                self.turn,
                first / 1000
            );
            self.ui.send(Update::SpeakingStarted { turn: self.turn });
        }
        if self.speech.resume.get() && self.speech.pending.get() {
            self.speech.resume.set(false);
            self.ui.send(Update::SpeakingStarted { turn: self.turn });
        }
        self.ui.send(Update::Level {
            turn: self.turn,
            level: output.meter.rms(),
        });
    }

    /// Moves speech to the device; true once there is nothing left to do.
    fn feed(&mut self) -> bool {
        let next_rate = self.speech.queue.borrow().front().map(|p| p.rate);
        let Some(output) = &mut self.output else {
            if self.open_at.is_some_and(|at| Instant::now() < at) {
                return false;
            }
            return match next_rate {
                Some(rate) => {
                    self.open(rate);
                    // Without a device there is nothing to wait for.
                    self.output.is_none()
                }
                None => self.speech.complete.get(),
            };
        };
        match next_rate {
            Some(rate) if rate != output.rate => {
                // The voice changed: play out what is written, then reopen.
                output.writer = None;
                if output.playback.is_finished() || output.playback.first_audio_micros().is_none() {
                    self.output = None;
                }
                false
            }
            Some(_) => match &mut output.writer {
                Some(writer) if !writer.is_closed() => {
                    fill(writer, &mut self.speech.queue.borrow_mut());
                    false
                }
                _ => self.device_failed(),
            },
            None if self.speech.complete.get() => {
                output.writer = None;
                output.playback.is_finished()
            }
            None => output.playback.is_finished() && self.device_failed(),
        }
    }

    /// The device stopped early. If it had played before it was probably
    /// unplugged: the rest goes to whatever is the default by then, and
    /// `false` is returned. Otherwise it is given up.
    fn device_failed(&mut self) -> bool {
        let why = match self.output.take().map(|o| o.playback.finish()) {
            Some(Err(err)) => err.to_string(),
            _ => "it stopped".to_owned(),
        };
        log::warn!("turn {}: the speech output failed: {why}", self.turn);
        if !self.speaking || self.reopened == MAX_REOPENS {
            return true;
        }
        self.reopened += 1;
        self.open_at = Some(Instant::now() + REOPEN_AFTER);
        false
    }
}

/// Writes as much queued speech as fits (of one sample rate).
fn fill(writer: &mut PlaybackWriter, queue: &mut VecDeque<Piece>) {
    let Some(rate) = queue.front().map(|p| p.rate) else {
        return;
    };
    while let Some(piece) = queue.front_mut().filter(|p| p.rate == rate) {
        piece.used += writer.push(&piece.samples[piece.used..]);
        if piece.used < piece.samples.len() {
            return;
        }
        queue.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use sohi_win::{AppWindow, EventQueue};

    use super::*;

    fn piece(rate: u32, len: usize) -> Piece {
        Piece {
            rate,
            samples: vec![1; len],
            used: 0,
        }
    }

    #[test]
    fn fill_writes_one_rate_and_keeps_the_rest() {
        let (mut writer, mut reader) = playback_ring(1_000);
        let mut queue = VecDeque::from([piece(1_000, 600), piece(1_000, 600), piece(2_000, 5)]);
        fill(&mut writer, &mut queue);
        assert_eq!(queue.len(), 2, "the ring holds 1000 samples");
        assert_eq!(queue[0].used, 400);
        reader.read(&mut [0; 1_000]);
        fill(&mut writer, &mut queue);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].rate, 2_000);
    }

    #[test]
    fn waiting_counts_what_is_left() {
        let mut half = piece(24_000, 24_000);
        half.used = 12_000;
        let queue = RefCell::new(VecDeque::from([half, piece(16_000, 16_000)]));
        assert_eq!(waiting(&queue), Duration::from_millis(1_500));
    }

    #[test]
    fn quiet_means_everything_was_said() {
        let speech = Speech::default();
        assert!(speech.is_quiet());
        speech.unsaid.set(1);
        assert!(!speech.is_quiet());
        speech.queue.borrow_mut().push_back(piece(1_000, 5));
        speech.unsaid.set(0);
        assert!(!speech.is_quiet());
        speech.queue.borrow_mut().clear();
        speech.pending.set(true);
        assert!(!speech.is_quiet());
        speech.pending.set(false);
        assert!(speech.is_quiet());
    }

    /// A device that stops while it plays is replaced, and the rest is
    /// heard there (audible: a quiet hum).
    #[tokio::test]
    #[ignore = "plays on the real output device"]
    async fn a_lost_device_is_replaced() {
        let window = AppWindow::create("Sohi.Test.Lost", 0xC142, EventQueue::new()).unwrap();
        let (ui, _updates) = crate::runtime::test_link(window.remote());
        let speech = Speech::default();
        // Longer than the ring holds, so some is left to be played.
        speech
            .queue
            .borrow_mut()
            .push_back(piece(24_000, 24_000 * 4));
        speech.complete.set(true);
        let mut speaker = Speaker::new(1, Instant::now(), &speech, &ui);
        let mut lost = false;
        for _ in 0..1_000 {
            tokio::time::sleep(TICK).await;
            speaker.report();
            if speaker.speaking && !lost {
                lost = true;
                speaker.output.as_ref().unwrap().playback.stop();
            }
            if speaker.feed() {
                break;
            }
        }
        assert!(lost);
        assert_eq!(speaker.reopened, 1);
        assert!(speech.queue.borrow().is_empty());
    }

    /// Offline speech end to end: synthesized by Windows, played on the
    /// real device, with no network involved (audible: one short word).
    #[tokio::test]
    #[ignore = "speaks aloud on the real output device"]
    async fn speaks_offline() {
        let window = AppWindow::create("Sohi.Test.Speak", 0xC140, EventQueue::new()).unwrap();
        let (ui, updates) = crate::runtime::test_link(window.remote());
        let voice = Voice::Offline(OfflineVoice {
            name: "Microsoft Zira".to_owned(),
        });
        let (tx, rx) = unbounded_channel();
        tx.send("Ready.".to_owned()).unwrap();
        drop(tx);
        let started = Instant::now();
        speak(1, voice, rx, started, &Speech::default(), &ui).await;
        println!("spoken in {:?}", started.elapsed());
        let updates: Vec<Update> = updates.try_iter().collect();
        assert!(matches!(
            updates.first(),
            Some(Update::SpeakingStarted { turn: 1 })
        ));
        let loudest = updates
            .iter()
            .filter_map(|u| match u {
                Update::Level { level, .. } => Some(*level),
                _ => None,
            })
            .fold(0.0, f32::max);
        println!("loudest level {loudest}");
        assert!(loudest > 0.02);
    }
}
