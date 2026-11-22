//! Interaction region tests.

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

fn listening(m: &mut Interaction) -> u64 {
    let effects = m.step(Event::VoiceWake);
    let State::Listening { turn } = m.state() else {
        panic!("not listening: {:?}", m.state());
    };
    assert!(effects.contains(&Effect::StartListening { turn }));
    turn
}

#[test]
fn a_voice_wake_reveals_and_listens() {
    let mut m = Interaction::new();
    let effects = m.step(Event::VoiceWake);
    assert_eq!(
        effects,
        [
            Effect::Reveal(RetractRule::NoSpeech),
            Effect::StartListening { turn: 1 },
            Effect::SetVisual(VisualState::Listening),
        ]
    );
}

#[test]
fn a_voice_turn_runs_through_to_a_reply() {
    let mut m = Interaction::new();
    let turn = listening(&mut m);
    assert_eq!(
        m.step(Event::SpeechStarted { turn }),
        [Effect::Keep(RetractRule::Working)]
    );
    m.step(Event::SpeechEnded { turn });
    assert_eq!(m.state(), State::Transcribing { turn });
    let effects = m.step(Event::Transcript {
        turn,
        text: " open the browser ".into(),
    });
    assert_eq!(
        effects,
        [
            Effect::ShowPill { focus: false },
            Effect::SetBubble("› open the browser\n".into()),
            Effect::SetVisual(VisualState::Thinking),
        ]
    );
    assert_eq!(m.state(), State::Thinking { turn });
    m.step(Event::LlmDone { turn });
    assert_eq!(m.state(), State::TextEntry);
}

#[test]
fn an_empty_transcript_says_so_and_retracts_soon() {
    let mut m = Interaction::new();
    let turn = listening(&mut m);
    m.step(Event::SpeechEnded { turn });
    let effects = m.step(Event::Transcript {
        turn,
        text: "  ".into(),
    });
    assert!(effects.contains(&Effect::SetBubble("Didn't catch that.".into())));
    assert!(effects.contains(&Effect::Keep(RetractRule::ReplyDone)));
}

#[test]
fn no_speech_retracts_and_the_hotkey_again_stops() {
    let mut m = Interaction::new();
    let turn = listening(&mut m);
    assert_eq!(
        m.step(Event::NoSpeech { turn }),
        [Effect::SetVisual(VisualState::Idle), Effect::Retract]
    );
    assert_eq!(m.state(), State::Idle);

    let turn = listening(&mut m);
    let effects = m.step(Event::VoiceWake);
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert!(effects.contains(&Effect::Retract));
    assert_eq!(m.state(), State::Idle);
}

#[test]
fn a_voice_wake_while_busy_barges_in() {
    let mut m = Interaction::new();
    let text_turn = submitted(&mut m, "hello");
    let effects = m.step(Event::VoiceWake);
    assert_eq!(effects[0], Effect::CancelTurn { turn: text_turn });
    assert!(effects.contains(&Effect::HidePill));
    assert!(!effects.contains(&Effect::Retract));
    let State::Listening { turn } = m.state() else {
        panic!("{:?}", m.state());
    };
    assert!(turn > text_turn);
}

#[test]
fn stale_voice_updates_are_ignored() {
    let mut m = Interaction::new();
    let old = listening(&mut m);
    m.step(Event::Escape);
    let turn = listening(&mut m);
    assert_eq!(m.step(Event::SpeechEnded { turn: old }), []);
    assert_eq!(m.step(Event::NoSpeech { turn: old }), []);
    assert_eq!(m.state(), State::Listening { turn });
}

#[test]
fn a_failed_voice_turn_shows_the_pill_with_the_error() {
    let mut m = Interaction::new();
    let turn = listening(&mut m);
    m.step(Event::SpeechEnded { turn });
    let effects = m.step(Event::TurnFailed {
        turn,
        message: "No connection.".into(),
    });
    assert_eq!(effects[0], Effect::ShowPill { focus: false });
    assert!(effects.contains(&Effect::AppendBubble("No connection.".into())));
    assert_eq!(m.state(), State::Error);
}

#[test]
fn escape_while_listening_cancels_and_retracts() {
    let mut m = Interaction::new();
    let turn = listening(&mut m);
    let effects = m.step(Event::Escape);
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert!(effects.contains(&Effect::Retract));
    assert_eq!(m.current_turn(), None);
}
