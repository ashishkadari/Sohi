//! Developer-only helpers: the `--dev-test-card` pulse animation and its
//! frame-cost statistics, and the `--dev-states` cycle through every
//! visual state. Nothing here is part of the final widget.

use std::time::{Duration, Instant};

use sohi_core::visual::VisualState;

/// How long `--dev-states` shows each state.
pub const STATE_HOLD: Duration = Duration::from_millis(2500);
/// When, into Thinking, `--dev-states` flashes an error.
pub const ERROR_AT: Duration = Duration::from_millis(1200);

/// Total length of the `--dev-states` cycle.
pub fn cycle_length() -> Duration {
    STATE_HOLD * u32::try_from(VisualState::ALL.len()).unwrap_or(8)
}

/// The state shown `elapsed` into the `--dev-states` cycle (the last one
/// once the cycle is over).
pub fn cycle_state(elapsed: Duration) -> VisualState {
    let index = usize::try_from(elapsed.as_millis() / STATE_HOLD.as_millis()).unwrap_or(usize::MAX);
    VisualState::ALL[index.min(VisualState::ALL.len() - 1)]
}

/// A speech-like 0–1 level for demoing Listening and Speaking.
pub fn fake_level(t: Duration) -> f32 {
    let t = t.as_secs_f32();
    ((t * 7.0).sin() * (t * 2.3).sin()).abs()
}

/// How long the test card pulses before going static.
pub const PULSE_DURATION: Duration = Duration::from_secs(3);

/// A 1 Hz brightness pulse on the test card, timing every frame it draws.
pub struct TestCardPulse {
    start: Instant,
    costs: Vec<Duration>,
}

impl TestCardPulse {
    /// Starts the pulse at `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            start: now,
            costs: Vec::with_capacity(128),
        }
    }

    /// Brightness level for `now`, or `None` once the pulse is over.
    pub fn level(&self, now: Instant) -> Option<u8> {
        let t = now.saturating_duration_since(self.start);
        (t < PULSE_DURATION).then(|| triangle_wave(t))
    }

    /// Records how long one frame took to render and present.
    pub fn record(&mut self, cost: Duration) {
        self.costs.push(cost);
    }

    /// Summary for the log: frame count and p50/p95/max cost.
    pub fn report(&self) -> String {
        let mut sorted = self.costs.clone();
        sorted.sort_unstable();
        format!(
            "{} frames; render+blit p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms (budget p95 <= 0.5 ms)",
            sorted.len(),
            ms(percentile(&sorted, 50)),
            ms(percentile(&sorted, 95)),
            ms(sorted.last().copied().unwrap_or_default()),
        )
    }
}

/// 0 -> 255 -> 0 once per second.
fn triangle_wave(t: Duration) -> u8 {
    let phase = t.as_millis() % 1000; // 0..1000
    let up = if phase < 500 { phase } else { 1000 - phase }; // 0..=500
    u8::try_from(up * 255 / 500).unwrap_or(u8::MAX)
}

/// Nearest-rank percentile of an ascending slice (zero when empty).
pub fn percentile(sorted: &[Duration], pct: usize) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_wave_spans_full_range() {
        assert_eq!(triangle_wave(Duration::ZERO), 0);
        assert_eq!(triangle_wave(Duration::from_millis(500)), 255);
        assert_eq!(triangle_wave(Duration::from_millis(1000)), 0);
        assert_eq!(triangle_wave(Duration::from_millis(250)), 127);
    }

    #[test]
    fn pulse_ends_after_duration() {
        let start = Instant::now();
        let pulse = TestCardPulse::new(start);
        assert!(pulse.level(start).is_some());
        assert!(pulse.level(start + PULSE_DURATION).is_none());
    }

    #[test]
    fn dev_cycle_visits_every_state_in_order() {
        for (i, state) in VisualState::ALL.into_iter().enumerate() {
            let at = STATE_HOLD * u32::try_from(i).unwrap() + STATE_HOLD / 2;
            assert_eq!(cycle_state(at), state);
        }
        assert_eq!(cycle_state(cycle_length() * 2), VisualState::ForcedOffline);
    }

    #[test]
    fn fake_level_stays_in_range() {
        for ms in (0..5000).step_by(7) {
            let v = fake_level(Duration::from_millis(ms));
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&v, 50), Duration::from_millis(50));
        assert_eq!(percentile(&v, 95), Duration::from_millis(95));
        assert_eq!(percentile(&[], 95), Duration::ZERO);
        let one = [Duration::from_millis(7)];
        assert_eq!(percentile(&one, 95), Duration::from_millis(7));
    }
}
