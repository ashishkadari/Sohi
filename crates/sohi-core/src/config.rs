//! Configuration schema (spec §5.7).
//!
//! The schema is pure: parsing takes a string and returns a value, and the
//! binary crate owns reading and writing `%APPDATA%\Sohi\config.toml`.
//! Every field has a default, so a partial file fills in the rest. Unknown
//! keys are ignored so that older builds tolerate newer files.

use serde::Deserialize;

/// The commented default config written on first run.
///
/// It must parse to exactly [`Config::default()`]; a unit test enforces this.
pub const DEFAULT_CONFIG_TOML: &str = include_str!("default_config.toml");

/// Error returned when a config file cannot be parsed.
#[derive(Debug, thiserror::Error)]
#[error("invalid config: {0}")]
pub struct ConfigError(#[from] toml::de::Error);

/// Root of the user configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Wake triggers.
    pub wake: WakeConfig,
    /// Widget presentation.
    pub ui: UiConfig,
    /// Language model routing.
    pub llm: LlmConfig,
    /// Speech-to-text.
    pub stt: SttConfig,
    /// Text-to-speech.
    pub tts: TtsConfig,
    /// How Sohi talks to the user.
    pub persona: PersonaConfig,
    /// Text-mode behaviour.
    pub text: TextConfig,
    /// Tool policy overrides.
    pub tools: ToolsConfig,
    /// Conversation history.
    pub history: HistoryConfig,
    /// Logging.
    pub log: LogConfig,
}

impl Config {
    /// Parses a config file's contents, filling missing fields with defaults.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the text is not valid TOML or a known key
    /// has the wrong type.
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }
}

/// `[wake]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct WakeConfig {
    /// Hotkey that toggles voice mode.
    pub hotkey_voice: String,
    /// Hotkey that opens text mode.
    pub hotkey_text: String,
    /// Side of the square hot zone in physical pixels.
    pub corner_size_px: u32,
    /// Required continuous dwell in the hot zone, in milliseconds.
    pub dwell_ms: u32,
    /// Minimum time after a retract before the corner can wake again.
    pub cooldown_ms: u32,
}

impl Default for WakeConfig {
    fn default() -> Self {
        Self {
            hotkey_voice: "Ctrl+Shift+Space".to_owned(),
            hotkey_text: "Ctrl+Alt+T".to_owned(),
            corner_size_px: 8,
            dwell_ms: 350,
            cooldown_ms: 1500,
        }
    }
}

/// Reduced-motion preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReducedMotion {
    /// Follow `SPI_GETCLIENTAREAANIMATION`.
    #[default]
    System,
    /// Always use the short fade.
    On,
    /// Always slide.
    Off,
}

/// `[ui]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Briefly reveal the widget after launch.
    pub launch_peek: bool,
    /// Reduced-motion preference.
    pub reduced_motion: ReducedMotion,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            launch_peek: true,
            reduced_motion: ReducedMotion::System,
        }
    }
}

/// `[persona]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PersonaConfig {
    /// Name Sohi calls the user; empty means no name.
    pub user_name: String,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            user_name: "Ashish".to_owned(),
        }
    }
}

/// `[llm]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Primary online provider.
    pub provider: String,
    /// Model for the primary provider.
    pub model: String,
    /// Online fallback provider.
    pub fallback_provider: String,
    /// Model for the Anthropic provider; empty disables it.
    pub anthropic_model: String,
    /// Ollama model used offline; empty disables Ollama.
    pub offline_model: String,
}

impl LlmConfig {
    /// The Anthropic model to fall back to, or `""` when there is no
    /// online fallback (another `fallback_provider`, or no model set).
    #[must_use]
    pub fn anthropic_fallback(&self) -> &str {
        if self
            .fallback_provider
            .trim()
            .eq_ignore_ascii_case("anthropic")
        {
            self.anthropic_model.trim()
        } else {
            ""
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "mistral".to_owned(),
            model: "mistral-small-latest".to_owned(),
            fallback_provider: "anthropic".to_owned(),
            anthropic_model: String::new(),
            offline_model: String::new(),
        }
    }
}

/// `[stt]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// Transcription provider.
    pub provider: String,
    /// Transcription model.
    pub model: String,
    /// Language hint, or `auto`.
    pub language: String,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            provider: "mistral".to_owned(),
            model: "voxtral-mini-latest".to_owned(),
            language: "auto".to_owned(),
        }
    }
}

/// `[tts]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// Online TTS provider.
    pub provider: String,
    /// Online voice ID; empty uses the provider default.
    pub voice: String,
    /// `OneCore` voice name, or `system-default`.
    pub offline_voice: String,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: "mistral".to_owned(),
            voice: String::new(),
            offline_voice: "system-default".to_owned(),
        }
    }
}

/// `[text]` section.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct TextConfig {
    /// Speak replies to typed prompts.
    pub speak_replies: bool,
}

/// `[tools]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Tools that always ask for confirmation.
    pub confirm: Vec<String>,
    /// Tools removed from the tool list.
    pub disabled: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            confirm: vec!["clipboard_get".to_owned()],
            disabled: Vec::new(),
        }
    }
}

/// `[history]` section.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// Keep history across restarts.
    pub persist: bool,
}

/// Log verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Normal operation.
    #[default]
    Info,
    /// Verbose; may include transcripts.
    Debug,
    /// Everything.
    Trace,
}

/// `[log]` section.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Minimum level written to the log file.
    pub level: LogLevel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_matches_default_struct() {
        let parsed = Config::from_toml_str(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn empty_file_is_all_defaults() {
        assert_eq!(Config::from_toml_str("").unwrap(), Config::default());
    }

    #[test]
    fn partial_file_fills_missing_fields() {
        let cfg =
            Config::from_toml_str("[wake]\ndwell_ms = 500\n[log]\nlevel = \"debug\"\n").unwrap();
        assert_eq!(cfg.wake.dwell_ms, 500);
        assert_eq!(cfg.wake.cooldown_ms, 1500);
        assert_eq!(cfg.log.level, LogLevel::Debug);
        assert_eq!(cfg.llm, LlmConfig::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let cfg = Config::from_toml_str("future = 1\n[wake]\nnew_thing = true\n").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn malformed_toml_is_an_error() {
        assert!(Config::from_toml_str("[wake\ndwell_ms = ").is_err());
    }

    #[test]
    fn wrong_type_is_an_error() {
        assert!(Config::from_toml_str("[wake]\ndwell_ms = \"long\"\n").is_err());
    }

    #[test]
    fn unknown_enum_value_is_an_error() {
        assert!(Config::from_toml_str("[ui]\nreduced_motion = \"sometimes\"\n").is_err());
    }

    #[test]
    fn the_anthropic_fallback_needs_a_model_and_the_provider() {
        let mut llm = LlmConfig::default();
        assert_eq!(llm.anthropic_fallback(), "");
        llm.anthropic_model = " claude-haiku-5-5 ".to_owned();
        assert_eq!(llm.anthropic_fallback(), "claude-haiku-5-5");
        llm.fallback_provider = "none".to_owned();
        assert_eq!(llm.anthropic_fallback(), "");
    }
}
