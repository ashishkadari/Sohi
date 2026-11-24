//! The conversation history sent with each turn (spec §5.5): a ring
//! buffer of the last answered turns, of which as many are sent as the
//! turn's [`Reach`] allows.
//!
//! Only answered turns are kept. The prompt in flight is held apart until
//! its reply arrives; if the turn fails or is cancelled the prompt is
//! dropped, so user and assistant messages always alternate (both chat
//! APIs expect that).

use std::collections::VecDeque;

use crate::chat::{Message, ToolCall, ToolRun};

/// How much of the conversation a turn is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Spoken turns and the pill: little, so the reply starts soon.
    Short,
    /// Full mode: long answers and code. Everything kept is in reach.
    Long,
}

impl Reach {
    /// How many answered turns at most.
    #[must_use]
    pub fn turns(self) -> usize {
        match self {
            Self::Short => 12,
            Self::Long => 200,
        }
    }

    /// About how many tokens at most, the new prompt included.
    #[must_use]
    pub fn tokens(self) -> usize {
        match self {
            Self::Short => 4_000,
            Self::Long => 24_000,
        }
    }
}

/// Longest call (arguments and result) that is remembered, in bytes.
const MAX_RUN: usize = 600;

/// A rough token count: about 4 characters per token, plus a few tokens
/// of per-message framing. Good enough for a cap; no tokenizer is bundled.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4) + 4
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Turn {
    user: String,
    /// The tools Sohi used. Without them it would read that "pause" was
    /// once answered with words alone, and do the same again.
    runs: Vec<ToolRun>,
    reply: String,
}

impl Turn {
    fn tokens(&self) -> usize {
        let runs = self.runs.iter().map(|run| {
            estimate_tokens(&run.call.arguments)
                + estimate_tokens(&run.call.name)
                + estimate_tokens(&run.result)
        });
        estimate_tokens(&self.user) + runs.sum::<usize>() + estimate_tokens(&self.reply)
    }

    /// Its messages. The calls get ids every provider accepts (Mistral
    /// takes nine letters and digits only), counted up in `calls`.
    fn write(&self, messages: &mut Vec<Message>, calls: &mut usize) {
        messages.push(Message::user(self.user.clone()));
        let runs: Vec<_> = self
            .runs
            .iter()
            .map(|run| {
                *calls += 1;
                let id = format!("call{calls:05}");
                (
                    ToolCall {
                        id,
                        ..run.call.clone()
                    },
                    &run.result,
                )
            })
            .collect();
        if !runs.is_empty() {
            let calls = runs.iter().map(|(call, _)| call.clone()).collect();
            messages.push(Message::calls("", calls, None));
            for (call, result) in runs {
                messages.push(Message::tool_result(call.id, result.clone()));
            }
        }
        messages.push(Message::assistant(self.reply.clone()));
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

    /// The reply to the pending prompt arrived, after the calls `runs`: keep
    /// them. Ignored if no prompt is pending. Very long calls are not kept.
    pub fn push_reply(&mut self, reply: impl Into<String>, mut runs: Vec<ToolRun>) {
        runs.retain(|run| run.call.arguments.len() + run.result.len() <= MAX_RUN);
        let Some(user) = self.pending.take() else {
            return;
        };
        self.turns.push_back(Turn {
            user,
            runs,
            reply: reply.into(),
        });
        while self.turns.len() > Reach::Long.turns() {
            self.turns.pop_front();
        }
        // Stored turns past the budget can never be sent; drop them now.
        while self.turns.len() > 1 && self.stored_tokens() > Reach::Long.tokens() {
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
    /// The oldest turns are left out until the whole is within `reach`;
    /// the newest turn and the prompt are always sent, so a follow-up to a
    /// long reply keeps its context.
    #[must_use]
    pub fn messages(&self, reach: Reach) -> Vec<Message> {
        let prompt = self.pending.as_deref();
        let mut used = prompt.map_or(0, estimate_tokens);
        let mut keep = 0;
        for turn in self.turns.iter().rev() {
            used += turn.tokens();
            if keep > 0 && (used > reach.tokens() || keep == reach.turns()) {
                break;
            }
            keep += 1;
        }
        let first = self.turns.len() - keep;
        let mut messages = Vec::with_capacity(keep * 2 + 1);
        let mut calls = 0;
        for turn in self.turns.iter().skip(first) {
            turn.write(&mut messages, &mut calls);
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
    use crate::chat::{Role, without_calls};

    fn answer(history: &mut History, user: &str, reply: &str) {
        history.push_user(user);
        history.push_reply(reply, Vec::new());
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
        let messages = history.messages(Reach::Short);
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
        assert_eq!(
            contents(&history.messages(Reach::Short)),
            ["Hi", "Hey.", "again"]
        );
    }

    #[test]
    fn a_new_prompt_replaces_an_unanswered_one() {
        let mut history = History::new();
        history.push_user("first");
        history.push_user("second");
        history.push_reply("ok", Vec::new());
        assert_eq!(contents(&history.messages(Reach::Short)), ["second", "ok"]);
    }

    #[test]
    fn a_reply_without_a_prompt_is_ignored() {
        let mut history = History::new();
        history.push_reply("stray", Vec::new());
        assert!(history.is_empty());
    }

    #[test]
    fn sends_the_last_twelve_turns() {
        let mut history = History::new();
        for i in 0..20 {
            answer(&mut history, &format!("q{i}"), &format!("a{i}"));
        }
        assert_eq!(history.len(), 20, "full mode still reaches them");
        let messages = history.messages(Reach::Short);
        assert_eq!(messages.len(), 24);
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
        let messages = history.messages(Reach::Short);
        let tokens: usize = messages.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(tokens <= Reach::Short.tokens(), "{tokens}");
        assert_eq!(messages.len(), 7);
        assert_eq!(messages[0].content, "q0");

        answer(&mut history, "q3", &long);
        answer(&mut history, "q4", &long);
        history.push_user("next");
        let messages = history.messages(Reach::Short);
        assert_eq!(messages[0].content, "q2");
        assert_eq!(history.messages(Reach::Long)[0].content, "q0");
    }

    #[test]
    fn full_mode_reaches_far_back() {
        let mut history = History::new();
        // About 5000 tokens of code, then a long talk about it.
        answer(&mut history, "code?", &"x".repeat(20_000));
        for i in 0..60 {
            answer(&mut history, &format!("q{i}"), &"y".repeat(1_000));
        }
        history.push_user("change the first line");
        assert_eq!(history.messages(Reach::Long)[0].content, "code?");
        assert_eq!(history.messages(Reach::Short).len(), 25);
    }

    #[test]
    fn what_no_turn_can_be_sent_is_not_stored() {
        let mut history = History::new();
        for i in 0..40 {
            answer(&mut history, &format!("q{i}"), &"x".repeat(4_000));
        }
        assert!(history.stored_tokens() <= Reach::Long.tokens());
        assert_eq!(history.len(), 23);
        for i in 0..300 {
            answer(&mut history, &format!("r{i}"), "ok");
        }
        assert_eq!(history.len(), Reach::Long.turns());
    }

    #[test]
    fn the_newest_turn_is_sent_even_if_it_is_huge() {
        let mut history = History::new();
        answer(&mut history, "old", "short");
        answer(&mut history, "essay", &"y".repeat(40_000));
        history.push_user("summarise that");
        let messages = history.messages(Reach::Short);
        assert_eq!(contents(&messages)[0], "essay");
        assert_eq!(messages.len(), 3);
        answer(&mut history, "book", &"z".repeat(100_000));
        assert_eq!(history.len(), 1);
    }

    fn run(arguments: &str) -> ToolRun {
        ToolRun {
            call: ToolCall {
                id: "toolu_1".to_owned(),
                name: "media".to_owned(),
                arguments: arguments.to_owned(),
            },
            result: "ok".to_owned(),
        }
    }

    #[test]
    fn the_tools_a_turn_used_are_sent_again() {
        let mut history = History::new();
        for _ in 0..2 {
            history.push_user("pause");
            history.push_reply("Paused.", vec![run("{}"), run("{}")]);
        }
        history.push_user("pause");
        let messages = history.messages(Reach::Short);
        let roles: Vec<_> = messages.iter().map(|m| m.role).collect();
        let turn = [
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Tool,
            Role::Assistant,
        ];
        assert_eq!(roles, [&turn[..], &turn[..], &[Role::User]].concat());
        let called: Vec<_> = messages.iter().flat_map(|m| &m.calls).collect();
        let answered: Vec<_> = messages.iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(called.len(), 4);
        for (i, (call, result)) in called.iter().zip(answered).enumerate() {
            assert_eq!(call.id, format!("call{:05}", i + 1));
            assert_eq!(call.id, result.call_id);
        }

        let plain = without_calls(&messages);
        let texts = ["pause", "Paused.", "pause", "Paused.", "pause"];
        assert_eq!(contents(&plain), texts);
    }

    #[test]
    fn a_very_long_call_is_not_kept() {
        let mut history = History::new();
        history.push_user("copy this");
        history.push_reply("Copied.", vec![run(&"x".repeat(MAX_RUN))]);
        assert_eq!(
            contents(&history.messages(Reach::Short)),
            ["copy this", "Copied."]
        );
    }

    #[test]
    fn clear_forgets_everything() {
        let mut history = History::new();
        answer(&mut history, "Hi", "Hey.");
        history.push_user("pending");
        history.clear();
        assert_eq!(history.messages(Reach::Short), []);
    }
}
