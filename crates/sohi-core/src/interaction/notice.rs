//! Notices: Sohi speaks first, because a reminder is due (spec §5.10).

use super::{Effect, Event, Interaction, Source, State};
use crate::visibility::RetractRule;
use crate::visual::VisualState;

impl Interaction {
    /// Whether a notice can be shown now: no turn is running and no error
    /// is being read.
    #[must_use]
    pub fn can_announce(&self) -> bool {
        matches!(self.state, State::Idle | State::TextEntry)
    }

    /// The transitions around a notice, or `None` for an event they do not
    /// cover.
    pub(super) fn notice_step(&mut self, event: &Event) -> Option<Vec<Effect>> {
        match (self.state, event) {
            (State::Idle | State::TextEntry, Event::Announce { text, aloud }) => {
                Some(self.announce(text, *aloud))
            }
            // Not followed by listening: nobody asked Sohi to.
            (
                State::Thinking { turn } | State::Speaking { turn },
                Event::PlaybackDone { turn: t },
            ) if turn == *t && self.source == Source::Notice => {
                self.state = State::TextEntry;
                Some(vec![
                    Effect::SetVisual(VisualState::Idle),
                    Effect::Keep(self.notice_rule()),
                ])
            }
            _ => None,
        }
    }

    fn announce(&mut self, text: &str, aloud: bool) -> Vec<Effect> {
        let parked = self.state == State::Idle;
        if parked {
            self.focused = false;
            self.has_input = false;
        }
        let rule = if aloud {
            RetractRule::Working
        } else {
            self.notice_rule()
        };
        let mut effects = vec![
            if parked {
                Effect::Reveal(rule)
            } else {
                Effect::Keep(rule)
            },
            Effect::ShowPill { focus: false },
            Effect::SetBubble(text.to_owned()),
        ];
        if aloud {
            let turn = self.next_turn;
            self.next_turn += 1;
            self.source = Source::Notice;
            self.state = State::Thinking { turn };
            let text = text.to_owned();
            effects.push(Effect::Say { turn, text });
        } else {
            self.state = State::TextEntry;
        }
        effects
    }

    /// Long enough to be seen by someone who looks up later; a user at the
    /// pill is left alone.
    fn notice_rule(&self) -> RetractRule {
        if self.focused || self.has_input {
            RetractRule::TextFocused
        } else {
            RetractRule::Notice
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announce(aloud: bool) -> Event {
        Event::Announce {
            text: "Reminder: tea.".to_owned(),
            aloud,
        }
    }

    #[test]
    fn it_is_shown_and_said_without_listening_afterwards() {
        let mut it = Interaction::new().with_follow_up(true);
        let text = "Reminder: tea.".to_owned();
        assert_eq!(
            it.step(announce(true)),
            [
                Effect::Reveal(RetractRule::Working),
                Effect::ShowPill { focus: false },
                Effect::SetBubble(text.clone()),
                Effect::Say { turn: 1, text },
            ]
        );
        assert!(it.voice_active(), "Esc stops it");
        it.step(Event::SpeakingStarted { turn: 1 });
        assert_eq!(it.state(), State::Speaking { turn: 1 });
        assert_eq!(
            it.step(Event::PlaybackDone { turn: 1 }),
            [
                Effect::SetVisual(VisualState::Idle),
                Effect::Keep(RetractRule::Notice),
            ]
        );
        assert_eq!(it.state(), State::TextEntry);
    }

    #[test]
    fn it_can_be_silent() {
        let mut it = Interaction::new();
        let effects = it.step(announce(false));
        assert!(effects.contains(&Effect::Reveal(RetractRule::Notice)));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Say { .. })));
        assert_eq!(it.state(), State::TextEntry);
    }

    #[test]
    fn it_does_not_take_the_pill_away_from_someone_typing() {
        let mut it = Interaction::new();
        it.step(Event::TextHotkey);
        let effects = it.step(announce(true));
        assert_eq!(effects[0], Effect::Keep(RetractRule::Working));
        let effects = it.step(Event::PlaybackDone { turn: 1 });
        assert!(effects.contains(&Effect::Keep(RetractRule::TextFocused)));
    }

    #[test]
    fn it_never_interrupts_a_turn() {
        let mut it = Interaction::new();
        it.step(Event::VoiceWake { focus: false });
        assert!(!it.can_announce());
        assert_eq!(it.step(announce(true)), []);
        assert_eq!(it.state(), State::Listening { turn: 1 });
    }

    #[test]
    fn esc_cancels_it() {
        let mut it = Interaction::new();
        it.step(announce(true));
        let effects = it.step(Event::Escape);
        assert!(effects.contains(&Effect::CancelTurn { turn: 1 }));
        assert_eq!(it.state(), State::Idle);
    }

    #[test]
    fn the_next_spoken_turn_is_followed_up_as_usual() {
        let mut it = Interaction::new().with_follow_up(true);
        it.step(announce(true));
        it.step(Event::PlaybackDone { turn: 1 });
        it.step(Event::VoiceWake { focus: false });
        it.step(Event::SpeechEnded { turn: 2 });
        it.step(Event::Transcript {
            turn: 2,
            text: "snooze".to_owned(),
        });
        it.step(Event::SpeakingStarted { turn: 2 });
        it.step(Event::PlaybackDone { turn: 2 });
        assert_eq!(it.state(), State::Listening { turn: 3 });
    }
}
