//! Conversation messages shared by the history and the chat providers.

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The person using Sohi.
    User,
    /// Sohi.
    Assistant,
    /// The result of a tool call.
    Tool,
}

impl Role {
    /// The role name both chat APIs use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// One message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Who wrote it.
    pub role: Role,
    /// Its text.
    pub content: String,
    /// The tools an assistant message calls.
    pub calls: Vec<ToolCall>,
    /// The call a tool message answers.
    pub call_id: String,
    /// A message with calls exactly as its provider wrote it (JSON), if
    /// that provider needs it back that way.
    pub blocks: Option<String>,
}

/// A tool call as a model wrote it, unchecked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The provider's id for it, which the result must carry.
    pub id: String,
    /// The tool's name.
    pub name: String,
    /// The arguments: JSON, if the model did it right.
    pub arguments: String,
}

impl Message {
    /// A message from the user.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            ..Self::assistant(content)
        }
    }

    /// A message from Sohi.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            calls: Vec::new(),
            call_id: String::new(),
            blocks: None,
        }
    }

    /// An assistant message that calls tools. See [`Message::blocks`].
    #[must_use]
    pub fn calls(content: impl Into<String>, calls: Vec<ToolCall>, blocks: Option<String>) -> Self {
        Self {
            calls,
            blocks,
            ..Self::assistant(content)
        }
    }

    /// The result of the call `call_id`.
    #[must_use]
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            call_id: call_id.into(),
            ..Self::assistant(content)
        }
    }
}

/// A tool as a model is told about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSpec {
    /// What the model calls it.
    pub name: &'static str,
    /// What it does.
    pub description: &'static str,
    /// The JSON Schema of its arguments, as JSON text.
    pub parameters: &'static str,
}
