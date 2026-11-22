//! The conversation history sent with each turn (spec §5.5): a ring
//! buffer of the last 12 answered turns, capped at about 4k tokens.
//!
//! Only answered turns are kept. The prompt in flight is held apart until
//! its reply arrives; if the turn fails or is cancelled the prompt is
//! dropped, so user and assistant messages always alternate (both chat
//! APIs expect that).

use std::collections::VecDeque;

use crate::chat::Message;

/// Answered turns kept.
pub const MAX_TURNS: usize = 12;
/// About how many tokens of history are sent, the new prompt included.
pub const TOKEN_BUDGET: usize = 4_000;

/// A rough token count: about 4 characters per token, plus a few tokens
/// of per-message framing. Good enough for a cap; no tokenizer is bundled.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4) + 4
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Turn {
    user: String,
    reply: String,
}

impl Turn {
    fn tokens(&self) -> usize {
        estimate_tokens(&self.user) + estimate_tokens(&self.reply)
    }
}

/// The conversation so far.
#[derive(Debug, Clone, Default)]
pub struct History {
    turns: VecDeque<Turn>,
    pending: Option<String>,
}

impl History {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a turn with the user's prompt. A prompt still waiting for its
    /// reply is replaced.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.pending = Some(text.into());
    }

    /// The reply to the pending prompt arrived: keep the pair. Ignored if no
    /// prompt is pending.
    pub fn push_reply(&mut self, reply: impl Into<String>) {
        let Some(user) = self.pending.take() else {
            return;
        };
        self.turns.push_back(Turn {
            user,
            reply: reply.into(),
        });
        while self.turns.len() > MAX_TURNS {
            self.turns.pop_front();
        }
        // Stored turns past the budget can never be sent; drop them now.
        while self.turns.len() > 1 && self.stored_tokens() > TOKEN_BUDGET {
            self.turns.pop_front();
        }
    }

    /// The pending turn failed or was cancelled: forget its prompt.
    pub fn drop_pending(&mut self) {
        self.pending = None;
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.turns.clear();
        self.pending = None;
    }

    /// Answered turns kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether no turn has been answered yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// The messages to send, oldest first, ending with the pending prompt.
    /// The oldest turns are left out until the whole fits
    /// [`TOKEN_BUDGET`]; the newest turn and the prompt are always sent,
    /// so a follow-up to a long reply keeps its context.
    #[must_use]
    pub fn messages(&self) -> Vec<Message> {
        let prompt = self.pending.as_deref();
        let mut used = prompt.map_or(0, estimate_tokens);
        let mut keep = 0;
        for turn in self.turns.iter().rev() {
            used += turn.tokens();
            if keep > 0 && used > TOKEN_BUDGET {
                break;
            }
            keep += 1;
        }
        let first = self.turns.len() - keep;
        let mut messages = Vec::with_capacity(keep * 2 + 1);
        for turn in self.turns.iter().skip(first) {
            messages.push(Message::user(turn.user.clone()));
            messages.push(Message::assistant(turn.reply.clone()));
        }
        if let Some(prompt) = prompt {
            messages.push(Message::user(prompt));
        }
        messages
    }

    fn stored_tokens(&self) -> usize {
        self.turns.iter().map(Turn::tokens).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Role;

    fn answer(history: &mut History, user: &str, reply: &str) {
        history.push_user(user);
        history.push_reply(reply);
    }

    fn contents(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|m| m.content.as_str()).collect()
    }

    #[test]
    fn estimates_about_four_characters_per_token() {
        assert_eq!(estimate_tokens(""), 4);
        assert_eq!(estimate_tokens("abcd"), 5);
        assert_eq!(estimate_tokens("abcde"), 6);
        // Characters, not bytes.
        assert_eq!(estimate_tokens("ééééé"), 6);
    }

    #[test]
    fn sends_answered_turns_then_the_prompt() {
        let mut history = History::new();
        answer(&mut history, "Hi", "Hey.");
        history.push_user("Time?");
        let messages = history.messages();
        assert_eq!(contents(&messages), ["Hi", "Hey.", "Time?"]);
        let roles: Vec<_> = messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    }

    #[test]
    fn a_failed_turn_leaves_no_trace() {
        let mut history = History::new();
        answer(&mut history, "Hi", "Hey.");
        history.push_user("lost");
        history.drop_pending();
        history.push_user("again");
        assert_eq!(contents(&history.messages()), ["Hi", "Hey.", "again"]);
    }

    #[test]
    fn a_new_prompt_replaces_an_unanswered_one() {
        let mut history = History::new();
        history.push_user("first");
        history.push_user("second");
        history.push_reply("ok");
        assert_eq!(contents(&history.messages()), ["second", "ok"]);
    }

    #[test]
    fn a_reply_without_a_prompt_is_ignored() {
        let mut history = History::new();
        history.push_reply("stray");
        assert!(history.is_empty());
    }

    #[test]
    fn keeps_the_last_twelve_turns() {
        let mut history = History::new();
        for i in 0..20 {
            answer(&mut history, &format!("q{i}"), &format!("a{i}"));
        }
        assert_eq!(history.len(), MAX_TURNS);
        let messages = history.messages();
        assert_eq!(messages.len(), MAX_TURNS * 2);
        assert_eq!(messages[0].content, "q8");
        assert_eq!(messages[23].content, "a19");
    }

    #[test]
    fn the_oldest_turns_give_way_to_the_budget() {
        let mut history = History::new();
        // Each turn is about 1000 tokens.
        let long = "x".repeat(3_984);
        for i in 0..3 {
            answer(&mut history, &format!("q{i}"), &long);
        }
        history.push_user("next");
        let messages = history.messages();
        let tokens: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(tokens <= TOKEN_BUDGET, "{tokens}");
        assert_eq!(messages.len(), 7);
        assert_eq!(messages[0].content, "q0");

        answer(&mut history, "q3", &long);
        answer(&mut history, "q4", &long);
        history.push_user("next");
        let messages = history.messages();
        assert_eq!(messages[0].content, "q2");
        assert_eq!(history.len(), 3, "turns past the budget are not stored");
    }

    #[test]
    fn the_newest_turn_is_sent_even_if_it_is_huge() {
        let mut history = History::new();
        answer(&mut history, "old", "short");
        answer(&mut history, "essay", &"y".repeat(40_000));
        history.push_user("summarise that");
        let messages = history.messages();
        assert_eq!(contents(&messages)[0], "essay");
        assert_eq!(messages.len(), 3);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut history = History::new();
        answer(&mut history, "Hi", "Hey.");
        history.push_user("pending");
        history.clear();
        assert_eq!(history.messages(), []);
    }
}
