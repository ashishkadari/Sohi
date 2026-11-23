//! The Interaction region of the state machine (spec §5.2): text turns,
//! and voice turns up to the reply. Speaking arrives with TTS in P5; until
//! then a voice turn's reply shows in the pill like a text reply.
//!
//! Pure: events in, effects out. The app carries the effects out on the
//! head, the pill and the runtime thread.
//!
//! Every turn has an id. Updates from a turn that is no longer current
//! (it was cancelled, or a newer one started) are ignored, so a late
//! delta can never leak into the next reply.

use std::time::Duration;

use crate::visibility::RetractRule;
use crate::visual::VisualState;

/// How long the error state shows before going back to text entry.
pub const ERROR_HOLD: Duration = Duration::from_millis(1200);

/// Where the conversation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing going on; the pill is closed.
    Idle,
    /// The pill is open, waiting for a prompt.
    TextEntry,
    /// The mic is open for a voice turn.
    Listening {
        /// The voice turn.
        turn: u64,
    },
    /// The utterance is being transcribed.
    Transcribing {
        /// The voice turn.
        turn: u64,
    },
    /// The model is answering (a text turn, or a voice turn after its
    /// transcript).
    Thinking {
        /// The running turn.
        turn: u64,
    },
    /// A turn failed; the message shows for [`ERROR_HOLD`].
    Error,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The text hotkey: opens the pill, or closes it if open.
    TextHotkey,
    /// The head was clicked: opens the pill or focuses it.
    HeadClicked,
    /// The pill gained or lost focus. `has_input` says whether the input
    /// holds text (which also keeps the head up, spec §4.4).
    PillFocus {
        /// Whether the pill has keyboard focus.
        focused: bool,
        /// Whether the input is non-empty.
        has_input: bool,
    },
    /// Enter in the pill.
    TextSubmitted(String),
    /// A voice wake: the voice hotkey, a corner dwell, the tray's Talk or
    /// a second launch. Toggles listening off when already listening.
    VoiceWake,
    /// The user started speaking.
    SpeechStarted {
        /// The voice turn.
        turn: u64,
    },
    /// The utterance ended; it is being transcribed.
    SpeechEnded {
        /// The voice turn.
        turn: u64,
    },
    /// Nobody spoke within 8 s; the mic is closed.
    NoSpeech {
        /// The voice turn.
        turn: u64,
    },
    /// What the user said (possibly nothing).
    Transcript {
        /// The voice turn.
        turn: u64,
        /// The transcript.
        text: String,
    },
    /// The next piece of a reply.
    LlmDelta {
        /// The turn it belongs to.
        turn: u64,
        /// The text.
        text: String,
    },
    /// A reply finished.
    LlmDone {
        /// The turn.
        turn: u64,
    },
    /// A turn failed; `message` is for the user.
    TurnFailed {
        /// The turn.
        turn: u64,
        /// What to show in the bubble.
        message: String,
    },
    /// [`ERROR_HOLD`] passed.
    ErrorShown,
    /// Esc (or Alt+F4 on the pill).
    Escape,
    /// The head retracted by itself (a retract rule fired).
    HeadGone,
}

/// What to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Reveal the head (if parked) under this rule.
    Reveal(RetractRule),
    /// Put this rule in force on a shown head.
    Keep(RetractRule),
    /// Retract the head.
    Retract,
    /// Show the pill, taking focus or not.
    ShowPill {
        /// Whether the pill takes keyboard focus.
        focus: bool,
    },
    /// Hide the pill.
    HidePill,
    /// Replace the bubble's text.
    SetBubble(String),
    /// Append to the bubble.
    AppendBubble(String),
    /// Start a text turn.
    StartTurn {
        /// Its id.
        turn: u64,
        /// The prompt.
        text: String,
    },
    /// Open the mic and run a voice turn on the runtime thread.
    StartListening {
        /// Its id.
        turn: u64,
    },
    /// Cancel the running turn.
    CancelTurn {
        /// Its id.
        turn: u64,
    },
    /// Change the head's visual state.
    SetVisual(VisualState),
    /// Flash the head amber twice.
    FlashError,
    /// Send [`Event::ErrorShown`] after [`ERROR_HOLD`].
    StartErrorTimer,
}

/// How a turn started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Typed into the pill.
    Typed,
    /// A voice wake.
    Wake,
    /// The mic reopened after a voice reply; the pill is open.
    FollowUp,
}

/// The Interaction region.
#[derive(Debug, Clone)]
pub struct Interaction {
    state: State,
    next_turn: u64,
    focused: bool,
    has_input: bool,
    /// How the latest turn started.
    source: Source,
    follow_up: bool,
}

impl Default for Interaction {
    fn default() -> Self {
        Self::new()
    }
}

impl Interaction {
    /// Starts idle.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            next_turn: 1,
            focused: false,
            has_input: false,
            source: Source::Typed,
            follow_up: false,
        }
    }

    /// Whether to listen again after a voice reply.
    #[must_use]
    pub fn with_follow_up(mut self, follow_up: bool) -> Self {
        self.follow_up = follow_up;
        self
    }

    /// Whether a voice turn is listening, transcribing or answering (Esc
    /// cancels it whatever window has the keyboard).
    #[must_use]
    pub fn voice_active(&self) -> bool {
        self.source != Source::Typed && self.current_turn().is_some()
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// The turn whose updates are still wanted, if one is running.
    #[must_use]
    pub fn current_turn(&self) -> Option<u64> {
        match self.state {
            State::Listening { turn } | State::Transcribing { turn } | State::Thinking { turn } => {
                Some(turn)
            }
            _ => None,
        }
    }

    /// Feeds one event and returns the effects, in order.
    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        use State::{Error, Idle, Listening, TextEntry, Thinking, Transcribing};
        if let Some(effects) = self.voice_step(&event) {
            return effects;
        }
        match (self.state, event) {
            (Idle, Event::TextHotkey | Event::HeadClicked) => self.open(),
            (state, Event::TextHotkey | Event::Escape) if state != Idle => self.close(true),
            (state, Event::HeadGone) if state != Idle => self.close(false),
            (TextEntry | Thinking { .. } | Error, Event::HeadClicked) => {
                vec![Effect::ShowPill { focus: true }]
            }
            (TextEntry, Event::TextSubmitted(text)) => self.submit(&text),
            (Thinking { turn }, Event::LlmDelta { turn: t, text }) if t == turn => {
                vec![Effect::AppendBubble(text)]
            }
            (Thinking { turn }, Event::LlmDone { turn: t }) if t == turn => {
                if self.follow_up
                    && self.source != Source::Typed
                    && !self.focused
                    && !self.has_input
                {
                    return self.listen_again();
                }
                self.state = TextEntry;
                vec![
                    Effect::SetVisual(VisualState::Idle),
                    Effect::Keep(self.entry_rule()),
                ]
            }
            (
                Listening { turn } | Transcribing { turn },
                Event::TurnFailed { turn: t, message },
            ) if t == turn => {
                let mut effects = vec![Effect::ShowPill { focus: false }];
                effects.extend(self.fail(message));
                effects
            }
            (Thinking { turn }, Event::TurnFailed { turn: t, message }) if t == turn => {
                self.fail(message)
            }
            (Error, Event::ErrorShown) => {
                self.state = TextEntry;
                vec![Effect::Keep(self.entry_rule())]
            }
            (state, Event::PillFocus { focused, has_input }) => {
                self.focused = focused;
                self.has_input = has_input;
                if state == TextEntry {
                    vec![Effect::Keep(self.entry_rule())]
                } else {
                    Vec::new()
                }
            }
            // Anything else (stale turn updates, Escape while idle, ...)
            // changes nothing.
            _ => Vec::new(),
        }
    }

    /// The voice transitions, or `None` for an event they do not cover.
    fn voice_step(&mut self, event: &Event) -> Option<Vec<Effect>> {
        use State::{Error, Idle, Listening, TextEntry, Thinking, Transcribing};
        Some(match (self.state, event) {
            (Idle, Event::VoiceWake) => self.listen(),
            // The hotkey again stops listening.
            (Listening { .. }, Event::VoiceWake) => self.close(true),
            // Barge-in: drop whatever is going on and listen again.
            (TextEntry | Transcribing { .. } | Thinking { .. } | Error, Event::VoiceWake) => {
                let mut effects = self.close(false);
                effects.extend(self.listen());
                effects
            }
            (Listening { turn }, Event::SpeechStarted { turn: t }) if turn == *t => {
                vec![Effect::Keep(RetractRule::Working)]
            }
            (Listening { turn }, Event::SpeechEnded { turn: t }) if turn == *t => {
                self.state = Transcribing { turn };
                vec![
                    Effect::SetVisual(VisualState::Transcribing),
                    Effect::Keep(RetractRule::Working),
                ]
            }
            (Listening { turn }, Event::NoSpeech { turn: t }) if turn == *t => {
                if self.source == Source::FollowUp {
                    // The last reply is still in the pill: leave it up.
                    return Some(self.stop_following_up());
                }
                self.state = Idle;
                vec![Effect::SetVisual(VisualState::Idle), Effect::Retract]
            }
            // Clicking into the pill during a follow-up: the user would
            // rather type.
            (
                Listening { turn },
                Event::PillFocus {
                    focused: true,
                    has_input,
                },
            ) if self.source == Source::FollowUp => {
                self.focused = true;
                self.has_input = *has_input;
                let mut effects = vec![Effect::CancelTurn { turn }];
                effects.extend(self.stop_following_up());
                effects
            }
            (Transcribing { turn }, Event::Transcript { turn: t, text }) if turn == *t => {
                self.heard(turn, text)
            }
            _ => return None,
        })
    }

    fn listen(&mut self) -> Vec<Effect> {
        let turn = self.start_voice_turn(Source::Wake);
        vec![
            Effect::Reveal(RetractRule::NoSpeech),
            Effect::StartListening { turn },
            Effect::SetVisual(VisualState::Listening),
        ]
    }

    /// Listens for a follow-up with the reply still in the pill. The mic's
    /// own 8 s limit ends it, so no retract rule runs meanwhile.
    fn listen_again(&mut self) -> Vec<Effect> {
        let turn = self.start_voice_turn(Source::FollowUp);
        vec![
            Effect::Keep(RetractRule::Working),
            Effect::StartListening { turn },
            Effect::SetVisual(VisualState::Listening),
        ]
    }

    fn start_voice_turn(&mut self, source: Source) -> u64 {
        let turn = self.next_turn;
        self.next_turn += 1;
        self.state = State::Listening { turn };
        self.source = source;
        turn
    }

    fn stop_following_up(&mut self) -> Vec<Effect> {
        self.state = State::TextEntry;
        vec![
            Effect::SetVisual(VisualState::Idle),
            Effect::Keep(self.entry_rule()),
        ]
    }

    /// The transcript is in: show it and let the model answer, or say
    /// nothing was heard. The pill shows without taking focus.
    fn heard(&mut self, turn: u64, text: &str) -> Vec<Effect> {
        self.focused = false;
        self.has_input = false;
        let text = text.trim();
        if text.is_empty() {
            self.state = State::TextEntry;
            return vec![
                Effect::ShowPill { focus: false },
                Effect::SetBubble("Didn't catch that.".to_owned()),
                Effect::SetVisual(VisualState::Idle),
                Effect::Keep(RetractRule::ReplyDone),
            ];
        }
        self.state = State::Thinking { turn };
        vec![
            Effect::ShowPill { focus: false },
            Effect::SetBubble(format!("› {text}\n")),
            Effect::SetVisual(VisualState::Thinking),
        ]
    }

    fn fail(&mut self, message: String) -> Vec<Effect> {
        self.state = State::Error;
        vec![
            Effect::AppendBubble(message),
            Effect::SetVisual(VisualState::Idle),
            Effect::FlashError,
            Effect::Keep(RetractRule::Error { pill_open: true }),
            Effect::StartErrorTimer,
        ]
    }

    fn open(&mut self) -> Vec<Effect> {
        self.state = State::TextEntry;
        // The pill opens focused, so it stays until Esc.
        self.focused = true;
        vec![
            Effect::Reveal(RetractRule::TextFocused),
            Effect::ShowPill { focus: true },
        ]
    }

    fn close(&mut self, retract: bool) -> Vec<Effect> {
        let mut effects = Vec::new();
        if let Some(turn) = self.current_turn() {
            effects.push(Effect::CancelTurn { turn });
            effects.push(Effect::SetVisual(VisualState::Idle));
        }
        effects.push(Effect::HidePill);
        if retract {
            effects.push(Effect::Retract);
        }
        self.state = State::Idle;
        self.has_input = false;
        effects
    }

    fn submit(&mut self, text: &str) -> Vec<Effect> {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return Vec::new();
        }
        let turn = self.next_turn;
        self.next_turn += 1;
        self.state = State::Thinking { turn };
        self.has_input = false;
        self.source = Source::Typed;
        vec![
            Effect::SetBubble(format!("› {text}\n")),
            Effect::StartTurn { turn, text },
            Effect::SetVisual(VisualState::Thinking),
            Effect::Keep(RetractRule::TextFocused),
        ]
    }

    /// The retract rule while the pill waits for input (spec §4.4).
    fn entry_rule(&self) -> RetractRule {
        if self.focused || self.has_input {
            RetractRule::TextFocused
        } else {
            RetractRule::TextReplyUnfocused
        }
    }
}

#[cfg(test)]
mod tests;
