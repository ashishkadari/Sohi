//! Text mode: the pill, the Interaction region and text turns (spec §4.6,
//! §5.2). Carries the region's effects out on the head, the pill and the
//! runtime thread.
//!
//! The pill window and the runtime thread are created on first use. The
//! pill is destroyed after 5 minutes hidden, to give its memory back.

use std::time::{Duration, Instant};

use sohi_core::geometry::{BASE_DPI, RectPx};
use sohi_core::history::History;
use sohi_core::interaction::{ERROR_HOLD, Effect, Event, Interaction, State};
use sohi_core::persona::text_system_prompt;
use sohi_core::pill::pill_origin;
use sohi_core::visibility::{Input, Phase};
use sohi_providers::ProviderError;
use sohi_win::{AppTimer, EventQueue, PillWindow, RemoteWindow, TimerHost, WinEvent};

use crate::chat::{ChatTurn, Request, Update};
use crate::head::Head;
use crate::runtime::Runtime;

/// App-window timer ids.
pub const TIMER_PILL_IDLE: usize = 1;
/// See [`TIMER_PILL_IDLE`].
pub const TIMER_ERROR_HOLD: usize = 2;

/// How long a hidden pill is kept before it is destroyed.
const PILL_IDLE: Duration = Duration::from_mins(5);

/// Settings text mode needs.
pub struct TextSettings {
    /// `[llm] model`.
    pub model: String,
    /// The Anthropic fallback model; empty for none.
    pub fallback_model: String,
    /// `[persona] user_name`.
    pub user_name: String,
}

/// Text mode's state.
pub struct TextMode {
    interaction: Interaction,
    pill: Option<PillWindow>,
    runtime: Option<Runtime<Request, Update>>,
    history: History,
    last_prompt: Option<String>,
    queue: EventQueue,
    window: RemoteWindow,
    timers: TimerHost,
    pill_idle: Option<AppTimer>,
    error_hold: Option<AppTimer>,
    head_rect: RectPx,
    dpi: u32,
    settings: TextSettings,
}

impl TextMode {
    /// Text mode with nothing created yet.
    pub fn new(
        queue: EventQueue,
        window: RemoteWindow,
        timers: TimerHost,
        settings: TextSettings,
    ) -> Self {
        Self {
            interaction: Interaction::new(),
            pill: None,
            runtime: None,
            history: History::new(),
            last_prompt: None,
            queue,
            window,
            timers,
            pill_idle: None,
            error_hold: None,
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
    }

    /// Handles the pill's own window events.
    pub fn on_pill(&mut self, event: WinEvent, head: &mut Head) {
        let Some(pill) = &self.pill else {
            return;
        };
        match event {
            WinEvent::PillSubmit => {
                let text = pill.take_input();
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
            let event = match update {
                Update::Delta { turn, text } => Event::LlmDelta { turn, text },
                Update::Done { turn, reply } => {
                    if self.is_current(turn) {
                        self.history.push_reply(reply);
                    }
                    Event::LlmDone { turn }
                }
                Update::Failed { turn, error } => {
                    if self.is_current(turn) {
                        self.history.drop_pending();
                    }
                    Event::TurnFailed {
                        turn,
                        message: user_message(&error),
                    }
                }
            };
            self.event(event, head);
        }
    }

    /// Whether `turn` is the one still waited for (older turns' updates
    /// are stale).
    fn is_current(&self, turn: u64) -> bool {
        self.interaction.state() == (State::Thinking { turn })
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
            TIMER_ERROR_HOLD => {
                self.error_hold = None;
                self.event(Event::ErrorShown, head);
                true
            }
            _ => false,
        }
    }

    /// Closes text mode if the head went away on its own (a retract rule
    /// fired, the voice hotkey, or the session locked).
    pub fn follow_head(&mut self, head: &mut Head) {
        let gone = matches!(head.phase(), Phase::Retracting | Phase::Parked);
        if gone && self.interaction.state() != State::Idle {
            self.event(Event::HeadGone, head);
        }
    }

    /// The head moved or the DPI changed: move the pill with it.
    pub fn set_layout(&mut self, head_rect: RectPx, dpi: u32) {
        self.head_rect = head_rect;
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
        match effect {
            Effect::Reveal(rule) => head.input(rule.reveal()),
            Effect::Keep(rule) => head.input(rule.input()),
            Effect::Retract => head.input(Input::Retract),
            Effect::ShowPill { focus } => self.show_pill(focus),
            Effect::HidePill => self.hide_pill(),
            Effect::SetBubble(text) => self.with_pill(|p| p.set_bubble(&text)),
            Effect::AppendBubble(text) => self.with_pill(|p| p.append_bubble(&text)),
            Effect::StartTurn { turn, text } => return self.start_turn(turn, text),
            Effect::CancelTurn { turn } => self.cancel_turn(turn),
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
                    pill.set_placeholder("Type a message…");
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
        let request = Request::Chat(ChatTurn {
            turn,
            model: self.settings.model.clone(),
            fallback_model: self.settings.fallback_model.clone(),
            system: text_system_prompt(&self.settings.user_name, &sohi_win::local_time()),
            messages: self.history.messages(),
        });
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

/// What the bubble says about a failure (spec §9.2: never raw errors).
fn user_message(error: &ProviderError) -> String {
    match error {
        ProviderError::MissingKey(name) => {
            format!("No API key is set. Run: sohi.exe --set-key {name}")
        }
        _ => "Something went wrong. See the log for details.".to_owned(),
    }
}
