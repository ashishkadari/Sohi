//! Asking before a tool runs (spec §5.6).

use super::{Effect, Event, Interaction, State};
use crate::answer::parse_answer;
use crate::visibility::RetractRule;
use crate::visual::VisualState;

impl Interaction {
    /// The transitions around a question, or `None` for an event they do
    /// not cover.
    pub(super) fn confirm_step(&mut self, event: &Event) -> Option<Vec<Effect>> {
        use State::{Confirming, Speaking, Thinking};
        Some(match (self.state, event) {
            (
                Thinking { turn } | Speaking { turn },
                Event::ToolQuestion {
                    turn: t,
                    ask,
                    summary,
                },
            ) if turn == *t => {
                self.state = Confirming { turn, ask: *ask };
                vec![
                    // Never takes the keyboard: an Enter meant for another
                    // window must not become a yes.
                    Effect::ShowPill { focus: false },
                    Effect::AppendBubble(question(summary)),
                    Effect::SetVisual(VisualState::Confirming),
                    Effect::Keep(RetractRule::Working),
                ]
            }
            // An empty Enter is a yes. Anything typed that is not a plain
            // yes is a no.
            (Confirming { turn, ask }, Event::TextSubmitted(text)) => {
                let yes = text.trim().is_empty() || parse_answer(text) == Some(true);
                self.answered(turn, ask, yes)
            }
            (Confirming { turn, ask }, Event::Escape) => self.answered(turn, ask, false),
            (Confirming { .. }, Event::HeadClicked) => vec![Effect::ShowPill { focus: true }],
            // Answered by voice, or too late.
            (
                Confirming { turn, ask },
                Event::ToolAnswered {
                    turn: t,
                    ask: a,
                    yes,
                },
            ) if turn == *t && ask == *a => self.resume(turn, *yes),
            _ => return None,
        })
    }

    fn answered(&mut self, turn: u64, ask: u32, yes: bool) -> Vec<Effect> {
        let mut effects = vec![Effect::AnswerTool { turn, ask, yes }];
        effects.extend(self.resume(turn, yes));
        effects
    }

    fn resume(&mut self, turn: u64, yes: bool) -> Vec<Effect> {
        self.state = State::Thinking { turn };
        let shown = if yes { "Yes.\n" } else { "No.\n" };
        vec![
            Effect::AppendBubble(shown.to_owned()),
            Effect::SetVisual(VisualState::Thinking),
        ]
    }
}

/// The question as the pill shows it (spec §5.6).
fn question(summary: &str) -> String {
    format!("\nRun: {summary}? Enter = Yes · Esc = No\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typed turn asking its first question.
    fn asking(m: &mut Interaction) -> u64 {
        m.step(Event::TextHotkey);
        m.step(Event::TextSubmitted("read my clipboard".to_owned()));
        let turn = m.current_turn().unwrap();
        let effects = m.step(Event::ToolQuestion {
            turn,
            ask: 1,
            summary: "read your clipboard".to_owned(),
        });
        assert_eq!(effects[0], Effect::ShowPill { focus: false });
        assert_eq!(
            effects[1],
            Effect::AppendBubble("\nRun: read your clipboard? Enter = Yes · Esc = No\n".to_owned())
        );
        assert_eq!(effects[2], Effect::SetVisual(VisualState::Confirming));
        assert_eq!(m.state(), State::Confirming { turn, ask: 1 });
        turn
    }

    fn typed(text: &str) -> Option<bool> {
        let mut m = Interaction::new();
        let turn = asking(&mut m);
        let effects = m.step(Event::TextSubmitted(text.to_owned()));
        assert_eq!(m.state(), State::Thinking { turn });
        effects.iter().find_map(|e| match e {
            Effect::AnswerTool { ask: 1, yes, .. } => Some(*yes),
            _ => None,
        })
    }

    #[test]
    fn enter_is_yes_and_other_text_is_no() {
        assert_eq!(typed(""), Some(true));
        assert_eq!(typed("  "), Some(true));
        assert_eq!(typed("yes"), Some(true));
        assert_eq!(typed("no"), Some(false));
        assert_eq!(typed("what does it say"), Some(false));
    }

    #[test]
    fn escape_is_no_and_the_turn_goes_on() {
        let mut m = Interaction::new();
        let turn = asking(&mut m);
        let effects = m.step(Event::Escape);
        assert!(effects.contains(&Effect::AnswerTool {
            turn,
            ask: 1,
            yes: false
        }));
        assert!(!effects.contains(&Effect::HidePill));
        assert_eq!(m.state(), State::Thinking { turn });
        // The next Esc closes as ever.
        assert!(m.step(Event::Escape).contains(&Effect::CancelTurn { turn }));
    }

    #[test]
    fn an_answer_from_the_runtime_resumes_the_turn() {
        for yes in [true, false] {
            let mut m = Interaction::new();
            let turn = asking(&mut m);
            let stale = [(turn, 2), (turn + 1, 1)];
            for (turn, ask) in stale {
                assert_eq!(m.step(Event::ToolAnswered { turn, ask, yes }), []);
            }
            let effects = m.step(Event::ToolAnswered { turn, ask: 1, yes });
            assert!(
                !effects
                    .iter()
                    .any(|e| matches!(e, Effect::AnswerTool { .. }))
            );
            assert!(effects.contains(&Effect::SetVisual(VisualState::Thinking)));
            assert_eq!(m.state(), State::Thinking { turn });
        }
    }

    #[test]
    fn a_question_can_be_cancelled_or_fail() {
        let mut m = Interaction::new();
        let turn = asking(&mut m);
        assert!(
            m.step(Event::TextHotkey)
                .contains(&Effect::CancelTurn { turn })
        );
        assert_eq!(m.state(), State::Idle);

        let turn = asking(&mut m);
        let message = "x".to_owned();
        m.step(Event::TurnFailed { turn, message });
        assert_eq!(m.state(), State::Error);

        let mut m = Interaction::new();
        let turn = asking(&mut m);
        let effects = m.step(Event::VoiceWake { focus: true });
        assert!(effects.contains(&Effect::CancelTurn { turn }));
        assert!(matches!(m.state(), State::Listening { .. }));
    }

    #[test]
    fn questions_of_other_turns_are_ignored() {
        let mut m = Interaction::new();
        let question = |turn| Event::ToolQuestion {
            turn,
            ask: 1,
            summary: String::new(),
        };
        assert_eq!(m.step(question(1)), []);
        m.step(Event::TextHotkey);
        m.step(Event::TextSubmitted("hi".to_owned()));
        let turn = m.current_turn().unwrap();
        assert_eq!(m.step(question(turn + 1)), []);
        assert_eq!(m.state(), State::Thinking { turn });
    }
}
