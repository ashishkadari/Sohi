//! Developer-only helpers: the `--dev-test-card` pulse animation and its
//! frame-cost statistics. Nothing here is part of the final widget.

use std::time::{Duration, Instant};

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
fn percentile(sorted: &[Duration], pct: usize) -> Duration {
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
    fn nearest_rank_percentiles() {
        let v: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile(&v, 50), Duration::from_millis(50));
        assert_eq!(percentile(&v, 95), Duration::from_millis(95));
        assert_eq!(percentile(&[], 95), Duration::ZERO);
        let one = [Duration::from_millis(7)];
        assert_eq!(percentile(&one, 95), Duration::from_millis(7));
    }
}
