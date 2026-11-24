//! Conversation messages shared by the history and the chat providers.

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The person using Sohi.
    User,
    /// Sohi.
    Assistant,
}

impl Role {
    /// The role name both chat APIs use.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
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
}

impl Message {
    /// A message from the user.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    /// A message from Sohi.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
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
