//! API keys: which ones Sohi stores, and a string type that never prints
//! its contents and is wiped when dropped (spec §5.7, §9.5).

use std::fmt;

/// A key Sohi keeps in Windows Credential Manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyName {
    /// Mistral (chat and speech).
    Mistral,
    /// Anthropic (the fallback chat provider).
    Anthropic,
    /// The cloud text-to-speech provider.
    Tts,
}

impl KeyName {
    /// Every key, in the order the help text lists them.
    pub const ALL: [Self; 3] = [Self::Mistral, Self::Anthropic, Self::Tts];

    /// Parses the name used on the command line (`--set-key mistral`),
    /// ignoring case.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|key| key.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// The command-line name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mistral => "mistral",
            Self::Anthropic => "anthropic",
            Self::Tts => "tts",
        }
    }

    /// The Credential Manager target name.
    #[must_use]
    pub fn target(self) -> &'static str {
        match self {
            Self::Mistral => "Sohi/mistral",
            Self::Anthropic => "Sohi/anthropic",
            Self::Tts => "Sohi/tts",
        }
    }
}

/// A secret string. `Debug` shows no content, and the bytes are zeroed
/// when it is dropped.
pub struct Secret(String);

impl Secret {
    /// Wraps a key as typed or pasted: surrounding whitespace is removed.
    /// Returns `None` if nothing is left.
    #[must_use]
    pub fn from_input(input: &str) -> Option<Self> {
        let key = input.trim();
        (!key.is_empty()).then(|| Self(key.to_owned()))
    }

    /// The secret itself. Never log or display it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(redacted)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        // Keeps the zeroing from being optimised away as a dead store.
        std::hint::black_box(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_parse_case_insensitively() {
        assert_eq!(KeyName::parse("Mistral"), Some(KeyName::Mistral));
        assert_eq!(KeyName::parse(" anthropic "), Some(KeyName::Anthropic));
        assert_eq!(KeyName::parse("TTS"), Some(KeyName::Tts));
        assert_eq!(KeyName::parse("openai"), None);
        assert_eq!(KeyName::parse(""), None);
    }

    #[test]
    fn targets_match_the_spec() {
        let targets: Vec<_> = KeyName::ALL.iter().map(|k| k.target()).collect();
        assert_eq!(targets, ["Sohi/mistral", "Sohi/anthropic", "Sohi/tts"]);
    }

    #[test]
    fn input_is_trimmed_and_empty_input_is_none() {
        let key = Secret::from_input("  sk-test-123\r\n").unwrap();
        assert_eq!(key.expose(), "sk-test-123");
        assert!(Secret::from_input(" \r\n").is_none());
    }

    #[test]
    fn debug_never_shows_the_secret() {
        let key = Secret::from_input("sk-test-123").unwrap();
        let shown = format!("{key:?}");
        assert!(!shown.contains("sk-test"), "{shown}");
    }
}
