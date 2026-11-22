//! The Interaction region of the state machine, text paths (spec §5.2).
//!
//! Pure: events in, effects out. The app carries the effects out on the
//! head, the pill and the runtime thread. Voice states arrive in P4/P5.
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
    /// A text turn is running.
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

/// The Interaction region.
#[derive(Debug, Clone)]
pub struct Interaction {
    state: State,
    next_turn: u64,
    focused: bool,
    has_input: bool,
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
        }
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Feeds one event and returns the effects, in order.
    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        use State::{Error, Idle, TextEntry, Thinking};
        match (self.state, event) {
            (Idle, Event::TextHotkey | Event::HeadClicked) => self.open(),
            (TextEntry | Thinking { .. } | Error, Event::TextHotkey | Event::Escape) => {
                self.close(true)
            }
            (TextEntry | Thinking { .. } | Error, Event::HeadGone) => self.close(false),
            (TextEntry | Thinking { .. } | Error, Event::HeadClicked) => {
                vec![Effect::ShowPill { focus: true }]
            }
            (TextEntry, Event::TextSubmitted(text)) => self.submit(&text),
            (Thinking { turn }, Event::LlmDelta { turn: t, text }) if t == turn => {
                vec![Effect::AppendBubble(text)]
            }
            (Thinking { turn }, Event::LlmDone { turn: t }) if t == turn => {
                self.state = TextEntry;
                vec![
                    Effect::SetVisual(VisualState::Idle),
                    Effect::Keep(self.entry_rule()),
                ]
            }
            (Thinking { turn }, Event::TurnFailed { turn: t, message }) if t == turn => {
                self.state = Error;
                vec![
                    Effect::AppendBubble(message),
                    Effect::SetVisual(VisualState::Idle),
                    Effect::FlashError,
                    Effect::Keep(RetractRule::Error { pill_open: true }),
                    Effect::StartErrorTimer,
                ]
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
        if let State::Thinking { turn } = self.state {
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
mod tests {
    use super::*;

    fn submitted(m: &mut Interaction, text: &str) -> u64 {
        m.step(Event::TextHotkey);
        let effects = m.step(Event::TextSubmitted(text.to_owned()));
        match m.state() {
            State::Thinking { turn } => {
                assert!(effects.contains(&Effect::StartTurn {
                    turn,
                    text: text.trim().to_owned()
                }));
                turn
            }
            other => panic!("not thinking: {other:?}"),
        }
    }

    #[test]
    fn hotkey_opens_a_focused_pill_and_toggles_it_closed() {
        let mut m = Interaction::new();
        assert_eq!(
            m.step(Event::TextHotkey),
            [
                Effect::Reveal(RetractRule::TextFocused),
                Effect::ShowPill { focus: true }
            ]
        );
        assert_eq!(m.state(), State::TextEntry);
        assert_eq!(
            m.step(Event::TextHotkey),
            [Effect::HidePill, Effect::Retract]
        );
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn a_head_click_opens_or_refocuses() {
        let mut m = Interaction::new();
        assert_eq!(m.step(Event::HeadClicked).len(), 2);
        assert_eq!(
            m.step(Event::HeadClicked),
            [Effect::ShowPill { focus: true }]
        );
        assert_eq!(m.state(), State::TextEntry);
    }

    #[test]
    fn a_text_turn_streams_then_returns_to_entry() {
        let mut m = Interaction::new();
        let turn = submitted(&mut m, "  what time is it?  ");
        assert_eq!(
            m.step(Event::LlmDelta {
                turn,
                text: "Late.".into()
            }),
            [Effect::AppendBubble("Late.".into())]
        );
        assert_eq!(
            m.step(Event::LlmDone { turn }),
            [
                Effect::SetVisual(VisualState::Idle),
                Effect::Keep(RetractRule::TextFocused)
            ]
        );
        assert_eq!(m.state(), State::TextEntry);
    }

    #[test]
    fn submitting_shows_the_prompt_and_starts_thinking() {
        let mut m = Interaction::new();
        m.step(Event::TextHotkey);
        let effects = m.step(Event::TextSubmitted("hi".into()));
        assert_eq!(effects[0], Effect::SetBubble("› hi\n".into()));
        assert!(effects.contains(&Effect::SetVisual(VisualState::Thinking)));
    }

    #[test]
    fn blank_prompts_are_ignored() {
        let mut m = Interaction::new();
        m.step(Event::TextHotkey);
        assert_eq!(m.step(Event::TextSubmitted(" \t ".into())), []);
        assert_eq!(m.state(), State::TextEntry);
    }

    #[test]
    fn escape_mid_turn_cancels_and_late_updates_are_ignored() {
        let mut m = Interaction::new();
        let turn = submitted(&mut m, "long story");
        assert_eq!(
            m.step(Event::Escape),
            [
                Effect::CancelTurn { turn },
                Effect::SetVisual(VisualState::Idle),
                Effect::HidePill,
                Effect::Retract
            ]
        );
        let late = Event::LlmDelta {
            turn,
            text: "late".into(),
        };
        assert_eq!(m.step(late), []);
        assert_eq!(m.step(Event::LlmDone { turn }), []);
        assert_eq!(m.state(), State::Idle);
    }

    #[test]
    fn updates_from_an_older_turn_never_reach_a_newer_one() {
        let mut m = Interaction::new();
        let first = submitted(&mut m, "one");
        m.step(Event::Escape);
        let second = submitted(&mut m, "two");
        assert_ne!(first, second);
        let stale = Event::LlmDelta {
            turn: first,
            text: "old".into(),
        };
        assert_eq!(m.step(stale), []);
        assert_eq!(m.state(), State::Thinking { turn: second });
    }

    #[test]
    fn a_failure_shows_then_returns_to_entry() {
        let mut m = Interaction::new();
        let turn = submitted(&mut m, "hi");
        let effects = m.step(Event::TurnFailed {
            turn,
            message: "No key.".into(),
        });
        assert_eq!(effects[0], Effect::AppendBubble("No key.".into()));
        assert!(effects.contains(&Effect::FlashError));
        assert!(effects.contains(&Effect::StartErrorTimer));
        assert!(effects.contains(&Effect::Keep(RetractRule::Error { pill_open: true })));
        assert_eq!(m.state(), State::Error);
        assert_eq!(
            m.step(Event::ErrorShown),
            [Effect::Keep(RetractRule::TextFocused)]
        );
        assert_eq!(m.state(), State::TextEntry);
    }

    #[test]
    fn losing_focus_after_a_reply_starts_the_unfocused_timer() {
        let mut m = Interaction::new();
        let turn = submitted(&mut m, "hi");
        m.step(Event::LlmDone { turn });
        let blur = Event::PillFocus {
            focused: false,
            has_input: false,
        };
        assert_eq!(
            m.step(blur),
            [Effect::Keep(RetractRule::TextReplyUnfocused)]
        );
        // Typed text keeps it up even without focus.
        let typed = Event::PillFocus {
            focused: false,
            has_input: true,
        };
        assert_eq!(m.step(typed), [Effect::Keep(RetractRule::TextFocused)]);
    }

    #[test]
    fn focus_changes_mid_turn_do_not_retract() {
        let mut m = Interaction::new();
        submitted(&mut m, "hi");
        let blur = Event::PillFocus {
            focused: false,
            has_input: false,
        };
        assert_eq!(m.step(blur), []);
        assert!(matches!(m.state(), State::Thinking { .. }));
    }

    #[test]
    fn the_head_retracting_closes_the_pill_without_retracting_again() {
        let mut m = Interaction::new();
        submitted(&mut m, "hi");
        let effects = m.step(Event::HeadGone);
        assert!(matches!(effects[0], Effect::CancelTurn { .. }));
        assert!(effects.contains(&Effect::HidePill));
        assert!(!effects.contains(&Effect::Retract));
    }

    /// Every event in every state: nothing panics, and idle ignores
    /// everything except the two ways in.
    #[test]
    fn every_event_in_every_state_is_handled() {
        let events = || {
            vec![
                Event::TextHotkey,
                Event::HeadClicked,
                Event::PillFocus {
                    focused: true,
                    has_input: false,
                },
                Event::TextSubmitted("x".into()),
                Event::LlmDelta {
                    turn: 1,
                    text: "d".into(),
                },
                Event::LlmDone { turn: 1 },
                Event::TurnFailed {
                    turn: 1,
                    message: "m".into(),
                },
                Event::ErrorShown,
                Event::Escape,
                Event::HeadGone,
            ]
        };
        for event in events() {
            let mut m = Interaction::new();
            let opens = matches!(event, Event::TextHotkey | Event::HeadClicked);
            let effects = m.step(event.clone());
            assert_eq!(!effects.is_empty(), opens, "idle + {event:?}");
        }
        for setup in 0..3 {
            for event in events() {
                let mut m = Interaction::new();
                let turn = submitted(&mut m, "x");
                match setup {
                    0 => drop(m.step(Event::LlmDone { turn })),
                    1 => {}
                    _ => drop(m.step(Event::TurnFailed {
                        turn,
                        message: "m".into(),
                    })),
                }
                let _ = m.step(event);
            }
        }
    }
}
