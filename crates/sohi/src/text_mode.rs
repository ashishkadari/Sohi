//! The pill, the Interaction region and its turns, text and voice (spec
//! §4.6, §5.2). Carries the region's effects out on the head, the pill and
//! the runtime thread.
//!
//! The pill window and the runtime thread are created on first use. The
//! pill is destroyed after 5 minutes hidden, to give its memory back.

mod full;
mod net;

use std::rc::Rc;
use std::time::{Duration, Instant};

use sohi_core::config::{SttConfig, ToolsConfig, TtsConfig};
use sohi_core::connectivity::{Connectivity, Event as NetEvent};
use sohi_core::full::{Command, command};
use sohi_core::geometry::{BASE_DPI, RectPx};
use sohi_core::history::{History, Reach};
use sohi_core::interaction::{ERROR_HOLD, Effect, Event, Interaction, State};
use sohi_core::persona::{Channel, system_prompt};
use sohi_core::pill::pill_origin;
use sohi_core::visibility::{Input, Phase};
use sohi_core::visual::VisualState;
use sohi_tools::Policy;
use sohi_win::{
    AppTimer, EventQueue, HotkeyHost, HotkeyRegistration, MonitorInfo, PillWindow, RemoteWindow,
    TimerHost, WinEvent,
};

use crate::chat::{ChatTurn, ListenTurn, Request, Update};
use crate::full_mode::FullMode;
use crate::head::Head;
use crate::hotkeys;
use crate::runtime::Runtime;
use crate::ticker::Ticker;

/// App-window timer ids.
pub const TIMER_PILL_IDLE: usize = 1;
/// See [`TIMER_PILL_IDLE`].
pub const TIMER_ERROR_HOLD: usize = 2;
/// See [`TIMER_PILL_IDLE`].
pub const TIMER_BACKOFF: usize = 3;

/// How long a hidden pill is kept before it is destroyed.
const PILL_IDLE: Duration = Duration::from_mins(5);

/// An Enter this soon after a question appeared does not answer it.
const READ_FIRST: Duration = Duration::from_millis(400);

/// Settings text mode needs.
pub struct TextSettings {
    /// `[llm] model`.
    pub model: String,
    /// The Anthropic fallback model; empty for none.
    pub fallback_model: String,
    /// The Ollama model for offline turns; empty for none.
    pub offline_model: String,
    /// `[persona] user_name`.
    pub user_name: String,
    /// `[stt]`.
    pub stt: SttConfig,
    /// `[tts]`.
    pub tts: TtsConfig,
    /// `[tools]`.
    pub tools: ToolsConfig,
    /// `[wake] follow_up`.
    pub follow_up: bool,
    /// `[wake] hotkey_text`.
    pub hotkey_text: String,
    /// Start forced offline.
    pub forced_offline: bool,
}

/// Text mode's state.
pub struct TextMode {
    interaction: Interaction,
    pill: Option<PillWindow>,
    /// Full mode, while it is open.
    full: Option<FullMode>,
    ticker: Rc<Ticker>,
    runtime: Option<Runtime<Request, Update>>,
    history: History,
    last_prompt: Option<String>,
    policy: Policy,
    /// When the latest tool question was shown.
    asked: Instant,
    queue: EventQueue,
    window: RemoteWindow,
    timers: TimerHost,
    hotkeys: HotkeyHost,
    /// Esc, held while a voice turn runs: the mic is open but no Sohi
    /// window has the keyboard.
    escape: Option<HotkeyRegistration>,
    pill_idle: Option<AppTimer>,
    error_hold: Option<AppTimer>,
    net: Connectivity,
    backoff: Option<AppTimer>,
    /// When a wake found Sohi offline (see [`net`]).
    offline_wake: Option<Instant>,
    head_rect: RectPx,
    dpi: u32,
    settings: TextSettings,
}

impl TextMode {
    /// Text mode with nothing created yet.
    pub fn new(
        queue: EventQueue,
        ticker: Rc<Ticker>,
        window: RemoteWindow,
        timers: TimerHost,
        hotkeys: HotkeyHost,
        settings: TextSettings,
    ) -> Self {
        let net = Connectivity::new(settings.forced_offline);
        let mut interaction = Interaction::new().with_follow_up(settings.follow_up);
        interaction.set_voice_available(net.state().is_usable());
        Self {
            interaction,
            pill: None,
            full: None,
            ticker,
            runtime: None,
            history: History::new(),
            last_prompt: None,
            policy: Policy::new(&settings.tools),
            asked: Instant::now(),
            queue,
            window,
            timers,
            hotkeys,
            escape: None,
            pill_idle: None,
            error_hold: None,
            net,
            backoff: None,
            offline_wake: None,
            head_rect: RectPx::default(),
            dpi: BASE_DPI,
            settings,
        }
    }

    /// Feeds an Interaction event and carries out the effects, including
    /// any follow-up events they cause.
    pub fn event(&mut self, event: Event, head: &mut Head) {
        let mut pending = vec![event];
        while let Some(event) = pending.pop() {
            for effect in self.interaction.step(event) {
                pending.extend(self.apply(effect, head));
            }
        }
        self.sync_escape();
        self.settle_full(head);
    }

    /// Holds Esc exactly while a voice turn runs.
    fn sync_escape(&mut self) {
        if !self.interaction.voice_active() {
            self.escape = None;
        } else if self.escape.is_none() {
            self.escape = self
                .hotkeys
                .escape(hotkeys::ESCAPE)
                .inspect_err(|err| log::warn!("Esc cannot stop listening: {err}"))
                .ok();
        }
    }

    /// Handles the pill's own window events.
    pub fn on_pill(&mut self, event: WinEvent, head: &mut Head) {
        let Some(pill) = &self.pill else {
            return;
        };
        match event {
            // Too soon to have been read: this Enter was not an answer.
            WinEvent::PillSubmit
                if matches!(self.interaction.state(), State::Confirming { .. })
                    && self.asked.elapsed() < READ_FIRST => {}
            WinEvent::PillSubmit => {
                let text = pill.take_input();
                if command(&text) == Some(Command::Enter) {
                    return self.enter_full(head);
                }
                if !text.trim().is_empty() {
                    self.last_prompt = Some(text.clone());
                }
                self.event(Event::TextSubmitted(text), head);
            }
            WinEvent::PillEscape => self.event(Event::Escape, head),
            WinEvent::PillRecall => {
                if let Some(prompt) = &self.last_prompt {
                    pill.set_input(prompt);
                }
            }
            WinEvent::PillFocus(focused) => {
                let has_input = pill.has_input();
                pill.set_placeholder(&self.placeholder(focused));
                self.event(Event::PillFocus { focused, has_input }, head);
            }
            _ => {}
        }
    }

    /// Drains the runtime's updates.
    pub fn on_runtime(&mut self, head: &mut Head) {
        let updates: Vec<Update> = match &self.runtime {
            Some(runtime) => runtime.drain().collect(),
            None => return,
        };
        for update in updates {
            self.learn(&update, head);
            let event = match update {
                Update::Probed { .. } => continue,
                Update::Level { turn, level } => {
                    if self.is_current(turn) {
                        head.set_level(level);
                    }
                    continue;
                }
                Update::SpeechStarted { turn } => Event::SpeechStarted { turn },
                Update::SpeechEnded { turn } => Event::SpeechEnded { turn },
                Update::NoSpeech { turn } => Event::NoSpeech { turn },
                Update::Transcript { turn, text } => {
                    if self.is_current(turn) && !text.trim().is_empty() {
                        self.history.push_user(text.trim().to_owned());
                    }
                    Event::Transcript { turn, text }
                }
                Update::Command { turn, command } => {
                    if self.is_current(turn) {
                        self.obey(command, head);
                    }
                    continue;
                }
                Update::MicFailed { turn } => Event::TurnFailed {
                    turn,
                    message: "Sohi couldn't use the microphone.".to_owned(),
                },
                Update::Delta { turn, text } => Event::LlmDelta { turn, text },
                Update::Done { turn, reply, runs } => {
                    if self.is_current(turn) {
                        self.history.push_reply(reply, runs);
                    }
                    Event::LlmDone { turn }
                }
                Update::Question { turn, ask, summary } => {
                    self.asked = Instant::now();
                    Event::ToolQuestion { turn, ask, summary }
                }
                Update::Answered { turn, ask, yes } => Event::ToolAnswered { turn, ask, yes },
                Update::SpeakingStarted { turn } => Event::SpeakingStarted { turn },
                Update::PlaybackDone { turn } => Event::PlaybackDone { turn },
                Update::Failed { turn, error } => {
                    if self.is_current(turn) {
                        self.history.drop_pending();
                        // The message is not part of the reply.
                        self.end_full_reply();
                    }
                    Event::TurnFailed {
                        turn,
                        message: error.user_message(),
                    }
                }
            };
            self.event(event, head);
        }
    }

    /// Whether `turn` is the one still waited for (older turns' updates
    /// are stale).
    fn is_current(&self, turn: u64) -> bool {
        self.interaction.current_turn() == Some(turn)
    }

    /// Handles an app-window timer. Returns `false` if it is not ours.
    pub fn on_timer(&mut self, id: usize, head: &mut Head) -> bool {
        match id {
            TIMER_PILL_IDLE => {
                self.pill_idle = None;
                if self.pill.as_ref().is_some_and(|p| !p.is_visible()) {
                    self.pill = None;
                    log::info!("pill destroyed after {} s hidden", PILL_IDLE.as_secs());
                }
                true
            }
            TIMER_BACKOFF => {
                self.backoff = None;
                self.net(NetEvent::BackoffElapsed, head);
                true
            }
            TIMER_ERROR_HOLD => {
                self.error_hold = None;
                self.event(Event::ErrorShown, head);
                true
            }
            _ => false,
        }
    }

    /// Closes text mode if the head went away on its own (a retract rule
    /// fired, the voice hotkey, or the session locked), and tells
    /// Connectivity whether the head is out.
    pub fn follow_head(&mut self, head: &mut Head) {
        self.net_shown(self.is_full() || head.phase() != Phase::Parked, head);
        if self.is_full() {
            return;
        }
        let gone = matches!(head.phase(), Phase::Retracting | Phase::Parked);
        if gone && self.interaction.state() != State::Idle {
            self.event(Event::HeadGone, head);
        }
    }

    /// The head moved or the monitor changed: move the pill (and full
    /// mode) with it.
    pub fn set_layout(&mut self, head_rect: RectPx, monitor: MonitorInfo) {
        let dpi = monitor.dpi;
        self.head_rect = head_rect;
        if let Some(full) = self.full.as_mut() {
            full.set_layout(monitor, head_rect);
        }
        let dpi_changed = dpi != self.dpi;
        self.dpi = dpi;
        if let Some(pill) = &self.pill {
            if dpi_changed && let Err(err) = pill.set_dpi(dpi) {
                log::warn!("cannot rescale the pill: {err}");
            }
            let (x, y, _) = pill_origin(head_rect, dpi);
            pill.set_origin(x, y);
        }
    }

    /// Carries out one effect. Returns a follow-up event, if any.
    fn apply(&mut self, effect: Effect, head: &mut Head) -> Option<Event> {
        match self.full_effect(effect)? {
            Effect::Reveal(rule) => head.input(rule.reveal()),
            Effect::Keep(rule) => head.input(rule.input()),
            Effect::Retract => head.input(Input::Retract),
            Effect::ShowPill { focus } => self.show_pill(focus),
            Effect::HidePill => self.hide_pill(),
            Effect::SetBubble(text) => self.with_pill(|p| p.set_bubble(&text)),
            Effect::AppendBubble(text) => self.with_pill(|p| p.append_bubble(&text)),
            Effect::StartTurn { turn, text } => return self.start_turn(turn, text),
            Effect::StartListening { turn } => return self.start_listening(turn),
            Effect::CancelTurn { turn } => self.cancel_turn(turn),
            Effect::AnswerTool { turn, ask, yes } => self.answer_tool(turn, ask, yes),
            Effect::SetVisual(VisualState::Idle) => head.set_visual(self.base_visual()),
            Effect::SetVisual(visual) => head.set_visual(visual),
            Effect::FlashError => head.flash_error(),
            Effect::StartErrorTimer => {
                self.error_hold = self.start_timer(TIMER_ERROR_HOLD, ERROR_HOLD);
            }
        }
        None
    }

    fn with_pill(&self, f: impl FnOnce(&PillWindow)) {
        if let Some(pill) = &self.pill {
            f(pill);
        }
    }

    fn show_pill(&mut self, focus: bool) {
        self.pill_idle = None;
        if self.pill.is_none() {
            match PillWindow::create(self.queue.clone(), self.dpi) {
                Ok(pill) => {
                    self.pill = Some(pill);
                    log::info!("pill created");
                }
                Err(err) => {
                    log::error!("cannot create the pill: {err}");
                    return;
                }
            }
        }
        let (x, y, _) = pill_origin(self.head_rect, self.dpi);
        self.with_pill(|pill| {
            pill.set_placeholder(&self.placeholder(focus));
            pill.set_origin(x, y);
            pill.show(focus);
        });
    }

    fn hide_pill(&mut self) {
        if let Some(pill) = &self.pill {
            pill.hide();
            self.pill_idle = self.start_timer(TIMER_PILL_IDLE, PILL_IDLE);
        }
    }

    fn start_timer(&self, id: usize, delay: Duration) -> Option<AppTimer> {
        self.timers
            .start(id, delay)
            .inspect_err(|err| log::warn!("timer {id}: {err}"))
            .ok()
    }

    /// Hands the turn to the runtime thread, or returns its failure.
    fn start_turn(&mut self, turn: u64, text: String) -> Option<Event> {
        self.history.push_user(text);
        let channel = if self.is_full() {
            Channel::Full
        } else {
            Channel::Typed
        };
        let request = Request::Chat(self.chat_turn(channel, turn));
        let sent = self.runtime().map(|r| r.send(request));
        if matches!(sent, Some(Ok(()))) {
            return None;
        }
        log::error!("cannot start turn {turn}: {sent:?}");
        self.history.drop_pending();
        Some(Event::TurnFailed {
            turn,
            message: "Sohi could not start that request. Try again.".to_owned(),
        })
    }

    /// Hands a voice turn to the runtime thread, or returns its failure.
    /// The transcript joins the history when it arrives.
    fn start_listening(&mut self, turn: u64) -> Option<Event> {
        let request = Request::Listen(Box::new(ListenTurn {
            chat: self.chat_turn(Channel::Spoken, turn),
            stt: self.settings.stt.clone(),
            tts: self.settings.tts.clone(),
        }));
        let sent = self.runtime().map(|r| r.send(request));
        if matches!(sent, Some(Ok(()))) {
            return None;
        }
        log::error!("cannot start voice turn {turn}: {sent:?}");
        Some(Event::TurnFailed {
            turn,
            message: "Sohi could not start listening. Try again.".to_owned(),
        })
    }

    /// A chat turn over the history as it stands.
    fn chat_turn(&self, channel: Channel, turn: u64) -> ChatTurn {
        let online = self.net.state().is_usable();
        ChatTurn {
            turn,
            model: self.settings.model.clone(),
            fallback_model: self.settings.fallback_model.clone(),
            offline: (!online).then(|| self.settings.offline_model.clone()),
            system: system_prompt(
                channel,
                &self.settings.user_name,
                &sohi_win::local_time(),
                online,
            ),
            messages: self.history.messages(if self.is_full() {
                Reach::Long
            } else {
                Reach::Short
            }),
            policy: self.policy.clone(),
        }
    }

    fn cancel_turn(&mut self, turn: u64) {
        self.history.drop_pending();
        let Some(runtime) = &self.runtime else {
            return;
        };
        log::info!("turn {turn}: cancel requested");
        let request = Request::Cancel {
            turn,
            at: Instant::now(),
        };
        if let Err(err) = runtime.send(request) {
            // Its updates are ignored either way: the turn id moved on.
            log::warn!("cannot cancel turn {turn}: {err:?}");
        }
    }

    fn answer_tool(&self, turn: u64, ask: u32, yes: bool) {
        log::info!("turn {turn}: question {ask} answered {yes}");
        let Some(runtime) = &self.runtime else {
            return;
        };
        if let Err(err) = runtime.send(Request::Answer { turn, ask, yes }) {
            // Without an answer the question times out as a no.
            log::warn!("cannot answer question {ask}: {err:?}");
        }
    }

    fn runtime(&mut self) -> Option<&Runtime<Request, Update>> {
        if self.runtime.is_none() {
            match Runtime::start(self.window, crate::chat::serve) {
                Ok(runtime) => self.runtime = Some(runtime),
                Err(err) => log::error!("cannot start the runtime thread: {err}"),
            }
        }
        self.runtime.as_ref()
    }
}
