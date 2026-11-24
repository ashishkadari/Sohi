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
    let (turn, ask, yes) = (1, 1, true);
    let answered = Event::ToolAnswered { turn, ask, yes };
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
            Event::SpeakingStarted { turn: 1 },
            Event::PlaybackDone { turn: 1 },
            Event::TurnFailed {
                turn: 1,
                message: "m".into(),
            },
            Event::ToolQuestion {
                turn: 1,
                ask: 1,
                summary: "s".into(),
            },
            answered.clone(),
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
    for setup in 0..4 {
        for event in events() {
            let mut m = Interaction::new();
            let turn = submitted(&mut m, "x");
            match setup {
                0 => drop(m.step(Event::LlmDone { turn })),
                1 => {}
                2 => drop(m.step(Event::ToolQuestion {
                    turn,
                    ask: 1,
                    summary: "s".into(),
                })),
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
    let effects = m.step(Event::VoiceWake { focus: true });
    let State::Listening { turn } = m.state() else {
        panic!("not listening: {:?}", m.state());
    };
    assert!(effects.contains(&Effect::StartListening { turn }));
    turn
}

#[test]
fn a_voice_wake_reveals_and_listens() {
    let mut m = Interaction::new();
    let effects = m.step(Event::VoiceWake { focus: true });
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
    assert_eq!(m.step(Event::LlmDone { turn }), []);
    assert_eq!(m.state(), State::Thinking { turn });
    m.step(Event::PlaybackDone { turn });
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
    let effects = m.step(Event::VoiceWake { focus: true });
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert!(effects.contains(&Effect::Retract));
    assert_eq!(m.state(), State::Idle);
}

#[test]
fn a_voice_wake_while_busy_barges_in() {
    let mut m = Interaction::new();
    let text_turn = submitted(&mut m, "hello");
    let effects = m.step(Event::VoiceWake { focus: true });
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

/// A voice turn answered with follow-up on; returns the follow-up's turn.
fn following_up(m: &mut Interaction) -> u64 {
    let turn = listening(m);
    m.step(Event::SpeechEnded { turn });
    m.step(Event::Transcript {
        turn,
        text: "hello".into(),
    });
    m.step(Event::SpeakingStarted { turn });
    assert_eq!(m.step(Event::LlmDone { turn }), [], "still speaking");
    let effects = m.step(Event::PlaybackDone { turn });
    let next = turn + 1;
    assert_eq!(
        effects,
        [
            Effect::Keep(RetractRule::Working),
            Effect::StartListening { turn: next },
            Effect::SetVisual(VisualState::Listening),
        ]
    );
    assert_eq!(m.state(), State::Listening { turn: next });
    next
}

#[test]
fn a_voice_reply_listens_again_and_silence_leaves_the_reply_up() {
    let mut m = Interaction::new().with_follow_up(true);
    let turn = following_up(&mut m);
    assert!(m.voice_active());
    assert_eq!(
        m.step(Event::NoSpeech { turn }),
        [
            Effect::SetVisual(VisualState::Idle),
            Effect::Keep(RetractRule::TextReplyUnfocused),
        ]
    );
    assert_eq!(m.state(), State::TextEntry);
    assert!(!m.voice_active());
}

#[test]
fn clicking_into_the_pill_stops_the_follow_up() {
    let mut m = Interaction::new().with_follow_up(true);
    let turn = following_up(&mut m);
    let effects = m.step(Event::PillFocus {
        focused: true,
        has_input: false,
    });
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert!(effects.contains(&Effect::Keep(RetractRule::TextFocused)));
    assert_eq!(m.state(), State::TextEntry);
    m.step(Event::TextSubmitted("typed".into()));
    let State::Thinking { turn } = m.state() else {
        panic!("{:?}", m.state());
    };
    // A typed turn never opens the mic.
    assert!(!m.voice_active());
    m.step(Event::LlmDone { turn });
    assert_eq!(m.state(), State::TextEntry);
}

#[test]
fn a_focused_pill_gets_no_follow_up() {
    let mut m = Interaction::new().with_follow_up(true);
    let turn = listening(&mut m);
    m.step(Event::SpeechEnded { turn });
    m.step(Event::Transcript {
        turn,
        text: "hello".into(),
    });
    m.step(Event::PillFocus {
        focused: true,
        has_input: false,
    });
    m.step(Event::LlmDone { turn });
    m.step(Event::PlaybackDone { turn });
    assert_eq!(m.state(), State::TextEntry);
}

#[test]
fn escape_ends_a_follow_up() {
    let mut m = Interaction::new().with_follow_up(true);
    let turn = following_up(&mut m);
    let effects = m.step(Event::Escape);
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert!(effects.contains(&Effect::HidePill));
    assert_eq!(m.state(), State::Idle);
    // The next wake is a fresh one: silence retracts.
    let turn = listening(&mut m);
    assert!(m.step(Event::NoSpeech { turn }).contains(&Effect::Retract));
}

/// A voice turn whose reply is being spoken.
fn speaking(m: &mut Interaction) -> u64 {
    let turn = listening(m);
    m.step(Event::SpeechEnded { turn });
    m.step(Event::Transcript {
        turn,
        text: "hello".into(),
    });
    assert_eq!(
        m.step(Event::SpeakingStarted { turn }),
        [Effect::SetVisual(VisualState::Speaking)]
    );
    assert_eq!(m.state(), State::Speaking { turn });
    turn
}

#[test]
fn the_reply_keeps_arriving_while_spoken() {
    let mut m = Interaction::new();
    let turn = speaking(&mut m);
    let delta = Event::LlmDelta {
        turn,
        text: "More.".into(),
    };
    assert_eq!(m.step(delta), [Effect::AppendBubble("More.".into())]);
    assert_eq!(m.step(Event::LlmDone { turn }), []);
    assert_eq!(m.state(), State::Speaking { turn });
    assert_eq!(
        m.step(Event::PlaybackDone { turn }),
        [
            Effect::SetVisual(VisualState::Idle),
            Effect::Keep(RetractRule::ReplyDone)
        ]
    );
}

#[test]
fn a_wake_or_a_click_barges_in() {
    for event in [Event::VoiceWake { focus: true }, Event::HeadClicked] {
        let mut m = Interaction::new().with_follow_up(true);
        let turn = speaking(&mut m);
        let effects = m.step(event);
        assert_eq!(effects[0], Effect::CancelTurn { turn });
        assert!(effects.contains(&Effect::StartListening { turn: turn + 1 }));
        assert_eq!(m.state(), State::Listening { turn: turn + 1 });
        // What the old turn still says changes nothing.
        assert_eq!(m.step(Event::PlaybackDone { turn }), []);
    }
}

#[test]
fn a_click_during_a_typed_turn_shows_the_pill() {
    let mut m = Interaction::new();
    submitted(&mut m, "hi");
    assert_eq!(
        m.step(Event::HeadClicked),
        [Effect::ShowPill { focus: true }]
    );
}

/// Half-duplex (spec §5.5): the mic never opens while a reply is spoken,
/// unless the same step cancels the reply first.
#[test]
fn the_mic_stays_closed_while_speaking() {
    let events = |turn| {
        [
            Event::TextHotkey,
            Event::HeadClicked,
            Event::VoiceWake { focus: true },
            Event::TextSubmitted("t".into()),
            Event::SpeechStarted { turn },
            Event::SpeechEnded { turn },
            Event::NoSpeech { turn },
            Event::Transcript {
                turn,
                text: "x".into(),
            },
            Event::LlmDelta {
                turn,
                text: "x".into(),
            },
            Event::LlmDone { turn },
            Event::SpeakingStarted { turn },
            Event::TurnFailed {
                turn,
                message: "m".into(),
            },
            Event::ErrorShown,
            Event::PillFocus {
                focused: true,
                has_input: true,
            },
            Event::Escape,
            Event::HeadGone,
        ]
    };
    for event in events(1) {
        let mut m = Interaction::new().with_follow_up(true);
        let turn = speaking(&mut m);
        assert_eq!(turn, 1);
        let effects = m.step(event.clone());
        let cancelled = effects
            .iter()
            .position(|e| *e == Effect::CancelTurn { turn });
        let opened = effects
            .iter()
            .position(|e| matches!(e, Effect::StartListening { .. }));
        assert!(
            opened.is_none() || cancelled.is_some_and(|c| Some(c) < opened),
            "{event:?}: {effects:?}"
        );
    }
}

#[test]
fn typing_over_the_reply_replaces_it() {
    let mut m = Interaction::new();
    let turn = speaking(&mut m);
    assert_eq!(m.step(Event::TextSubmitted(" ".into())), []);
    let effects = m.step(Event::TextSubmitted("instead".into()));
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert_eq!(m.state(), State::Thinking { turn: turn + 1 });
    assert!(!m.voice_active());
}

#[test]
fn escape_stops_speaking() {
    let mut m = Interaction::new().with_follow_up(true);
    let turn = speaking(&mut m);
    assert!(m.voice_active());
    let effects = m.step(Event::Escape);
    assert_eq!(effects[0], Effect::CancelTurn { turn });
    assert!(effects.contains(&Effect::Retract));
    assert_eq!(m.state(), State::Idle);
}

#[test]
fn an_offline_wake_opens_the_pill() {
    for (focus, rule) in [
        (true, RetractRule::TextFocused),
        (false, RetractRule::TextReplyUnfocused),
    ] {
        let mut m = Interaction::new().with_follow_up(true);
        m.set_voice_available(false);
        assert_eq!(
            m.step(Event::VoiceWake { focus }),
            [Effect::Reveal(rule), Effect::ShowPill { focus }]
        );
        assert_eq!(m.state(), State::TextEntry);
        // Again: only brings the pill forward.
        assert_eq!(
            m.step(Event::VoiceWake { focus: true }),
            [Effect::ShowPill { focus: true }]
        );
        // Back online, a wake listens.
        m.set_voice_available(true);
        let effects = m.step(Event::VoiceWake { focus: true });
        assert!(effects.contains(&Effect::StartListening { turn: 1 }));
    }
}

#[test]
fn going_offline_during_a_reply_stops_the_follow_up() {
    let mut m = Interaction::new().with_follow_up(true);
    let turn = speaking(&mut m);
    m.set_voice_available(false);
    m.step(Event::LlmDone { turn });
    m.step(Event::PlaybackDone { turn });
    assert_eq!(m.state(), State::TextEntry);
}

#[test]
fn an_offline_wake_during_a_reply_stops_it_and_opens_the_pill() {
    for event in [Event::VoiceWake { focus: true }, Event::HeadClicked] {
        let mut m = Interaction::new();
        let turn = speaking(&mut m);
        m.set_voice_available(false);
        let effects = m.step(event);
        assert_eq!(effects[0], Effect::CancelTurn { turn });
        assert_eq!(effects.last(), Some(&Effect::ShowPill { focus: true }));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::StartListening { .. }))
        );
    }
}
