//! Replies that are spoken (spec §5.5).

use super::{Effect, Event, Interaction, Source, State};
use crate::visibility::RetractRule;
use crate::visual::VisualState;

impl Interaction {
    /// The transitions around a spoken reply, or `None` for an event they
    /// do not cover.
    pub(super) fn speech_step(&mut self, event: &Event) -> Option<Vec<Effect>> {
        use State::{Speaking, Thinking};
        Some(match (self.state, event) {
            (Thinking { turn }, Event::SpeakingStarted { turn: t }) if turn == *t => {
                self.state = Speaking { turn };
                vec![Effect::SetVisual(VisualState::Speaking)]
            }
            // A voice turn lasts until its reply was heard.
            (Thinking { turn }, Event::LlmDone { turn: t })
                if turn == *t && self.source != Source::Typed =>
            {
                Vec::new()
            }
            (Thinking { turn } | Speaking { turn }, Event::PlaybackDone { turn: t })
                if turn == *t =>
            {
                self.spoken()
            }
            // Typing over the reply replaces it.
            (Speaking { turn }, Event::TextSubmitted(text)) if !text.trim().is_empty() => {
                let mut effects = vec![Effect::CancelTurn { turn }];
                effects.extend(self.submit(text));
                effects
            }
            (Speaking { .. }, Event::TextSubmitted(_)) => Vec::new(),
            _ => return None,
        })
    }

    /// The transcript is in: show it and let the model answer, or say
    /// nothing was heard. The pill shows without taking focus.
    pub(super) fn heard(&mut self, turn: u64, text: &str) -> Vec<Effect> {
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

    /// A voice turn's reply was heard: listen for a follow-up, unless the
    /// user is at the pill.
    fn spoken(&mut self) -> Vec<Effect> {
        let at_pill = self.focused || self.has_input;
        if self.voice.follow_up && self.voice.available && !at_pill {
            return self.listen_again();
        }
        let heard = matches!(self.state, State::Speaking { .. });
        self.state = State::TextEntry;
        let rule = if heard && !at_pill {
            RetractRule::ReplyDone
        } else {
            self.entry_rule()
        };
        vec![Effect::SetVisual(VisualState::Idle), Effect::Keep(rule)]
    }
}
