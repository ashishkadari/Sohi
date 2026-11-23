//! The mic level that drives the Listening visor (spec §4.2): RMS mapped
//! onto a dB scale, then smoothed with a 30 ms attack and 150 ms release.

use std::time::Duration;

/// How fast the level rises.
pub const ATTACK: Duration = Duration::from_millis(30);
/// How fast it falls.
pub const RELEASE: Duration = Duration::from_millis(150);
/// Quieter than this (dBFS) shows as 0.
const FLOOR_DB: f32 = -50.0;
/// Louder than this (dBFS) shows as 1.
const CEILING_DB: f32 = -10.0;

/// Maps an RMS level (0 to 1 of full scale) to 0–1 for display:
/// −50 dBFS and below is 0, −10 dBFS and above is 1, linear in dB between.
#[must_use]
pub fn display_level(rms: f32) -> f32 {
    let db = 20.0 * rms.max(1e-6).log10();
    ((db - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// A one-pole smoother with separate attack and release.
#[derive(Debug, Clone, Copy, Default)]
pub struct Envelope {
    value: f32,
}

impl Envelope {
    /// Moves toward `target` over `dt` and returns the new value.
    pub fn step(&mut self, target: f32, dt: Duration) -> f32 {
        let tau = if target > self.value { ATTACK } else { RELEASE };
        let alpha = 1.0 - (-dt.as_secs_f32() / tau.as_secs_f32()).exp();
        self.value += (target - self.value) * alpha;
        self.value
    }

    /// Drops straight to 0.
    pub fn reset(&mut self) {
        self.value = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_decibels_onto_zero_to_one() {
        assert!(display_level(0.0).abs() < f32::EPSILON);
        assert!(display_level(0.001).abs() < 1e-6); // -60 dBFS
        assert!((display_level(1.0) - 1.0).abs() < f32::EPSILON);
        let mid = display_level(0.01); // -40 dBFS
        assert!((mid - 0.25).abs() < 1e-4, "{mid}");
    }

    #[test]
    fn rises_fast_and_falls_slowly() {
        let frame = Duration::from_millis(16);
        let mut envelope = Envelope::default();
        // One time constant of attack reaches about 63 %.
        let up = envelope.step(1.0, ATTACK);
        assert!((up - 0.632).abs() < 0.01, "{up}");
        for _ in 0..20 {
            envelope.step(1.0, frame);
        }
        let down = envelope.step(0.0, frame);
        // 16 ms of a 150 ms release loses about 10 %.
        assert!(down > 0.85 && down < 0.95, "{down}");
        envelope.reset();
        assert!(envelope.step(0.0, frame).abs() < f32::EPSILON);
    }
}
