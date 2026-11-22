//! What the head shows (spec §4.5). The state machine picks a
//! [`VisualState`]; `sohi-render` turns it into pixels.

use std::time::Duration;

/// The head's visual state. Errors are a short overlay on top of the
/// current state (see [`ERROR_FLASH`]), not a state of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VisualState {
    /// Waiting; breathing slowly.
    #[default]
    Idle,
    /// Mic open; the eyes follow the mic level.
    Listening,
    /// Speech sent for transcription; a scanline sweeps the frame.
    Transcribing,
    /// Waiting on the model; the corner brackets chase.
    Thinking,
    /// Talking; the collar line follows the voice envelope.
    Speaking,
    /// Waiting for the user to confirm a tool call; amber frame, blinking eyes.
    Confirming,
    /// No network.
    Offline,
    /// Offline because the user forced it.
    ForcedOffline,
}

impl VisualState {
    /// Every state, in spec order (for snapshots and the dev cycle).
    pub const ALL: [Self; 8] = [
        Self::Idle,
        Self::Listening,
        Self::Transcribing,
        Self::Thinking,
        Self::Speaking,
        Self::Confirming,
        Self::Offline,
        Self::ForcedOffline,
    ];

    /// Frames per second while revealed, or `None` for a static state that
    /// draws once.
    #[must_use]
    pub fn fps(self) -> Option<u32> {
        match self {
            Self::Idle | Self::Confirming => Some(30),
            Self::Listening | Self::Transcribing | Self::Thinking | Self::Speaking => Some(60),
            Self::Offline | Self::ForcedOffline => None,
        }
    }

    /// Lower-case name (logs, snapshot file names, `--dev-state`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Listening => "listening",
            Self::Transcribing => "transcribing",
            Self::Thinking => "thinking",
            Self::Speaking => "speaking",
            Self::Confirming => "confirming",
            Self::Offline => "offline",
            Self::ForcedOffline => "forced-offline",
        }
    }

    /// The state with this [`name`](Self::name).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Length of the error overlay: two 120 ms amber flashes with a 120 ms gap.
pub const ERROR_FLASH: Duration = Duration::from_millis(360);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for s in VisualState::ALL {
            assert_eq!(VisualState::from_name(s.name()), Some(s));
        }
        assert_eq!(VisualState::from_name("nope"), None);
    }

    #[test]
    fn offline_states_are_static() {
        assert_eq!(VisualState::Offline.fps(), None);
        assert_eq!(VisualState::ForcedOffline.fps(), None);
        assert_eq!(VisualState::Idle.fps(), Some(30));
        assert_eq!(VisualState::Speaking.fps(), Some(60));
    }
}
