//! The Visibility region of the state machine (spec §5.1, §4.3, §4.4).
//!
//! Pure: the caller passes the current time with every input and carries
//! out the returned [`Effect`]. The machine decides *whether* the head is
//! revealing, shown, retracting or parked, and when an automatic retract is
//! due; the caller owns the animation itself and reports
//! [`Input::SlideDone`] when a slide finishes.
//!
//! Events that arrive mid-animation reverse the slide: a reveal request
//! while retracting slides back in, a retract request while revealing slides
//! back out, each from wherever the head currently is.

use std::time::{Duration, Instant};

/// How long auto-retract waits after the pointer leaves the head (§4.4).
pub const HOVER_RESUME: Duration = Duration::from_secs(1);

/// Where the head is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Hidden; nothing renders.
    Parked,
    /// Sliding in.
    Revealing,
    /// Fully shown.
    Shown,
    /// Sliding out.
    Retracting,
}

/// Something that happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Show the head; once shown, retract automatically after
    /// `retract_after` (`None`: stay until told otherwise).
    Reveal {
        /// Auto-retract delay, counted from when the head is fully shown.
        retract_after: Option<Duration>,
    },
    /// Hide the head now.
    Retract,
    /// The current slide finished.
    SlideDone,
    /// The pointer entered (`true`) or left (`false`) the head.
    Hover(bool),
    /// Time passed; retracts if the auto-retract deadline is due.
    Tick,
    /// (Re)arm auto-retract `after` from now (or from when the reveal ends).
    ScheduleRetract(Duration),
    /// Cancel any auto-retract (e.g. text mode has focus).
    KeepShown,
}

/// What the caller must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Show the window if hidden and slide in from the current position.
    SlideIn,
    /// Slide out from the current position.
    SlideOut,
    /// Hide the window: the head is parked.
    Hide,
}

/// The auto-retract timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deadline {
    /// No automatic retract.
    None,
    /// Retract at this instant.
    At(Instant),
    /// Paused by hover with this much time left.
    Paused(Duration),
}

/// The Visibility region.
#[derive(Debug, Clone)]
pub struct Visibility {
    phase: Phase,
    /// Applies while shown.
    deadline: Deadline,
    /// Delay to arm when a reveal finishes.
    pending: Option<Duration>,
    hovered: bool,
    /// When the head last finished retracting.
    parked_at: Option<Instant>,
}

impl Default for Visibility {
    fn default() -> Self {
        Self::new()
    }
}

impl Visibility {
    /// Starts parked.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Phase::Parked,
            deadline: Deadline::None,
            pending: None,
            hovered: false,
            parked_at: None,
        }
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// When the machine next needs a [`Input::Tick`] (the auto-retract
    /// deadline), if any.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.phase, self.deadline) {
            (Phase::Shown, Deadline::At(t)) => Some(t),
            _ => None,
        }
    }

    /// Whether a corner wake may reveal now: parked, and at least
    /// `cooldown` since the last retract finished (§4.7).
    #[must_use]
    pub fn corner_wake_allowed(&self, now: Instant, cooldown: Duration) -> bool {
        self.phase == Phase::Parked
            && self
                .parked_at
                .is_none_or(|at| now.saturating_duration_since(at) >= cooldown)
    }

    /// Applies `input` at time `now`.
    pub fn step(&mut self, now: Instant, input: Input) -> Option<Effect> {
        if let Input::Hover(over) = input {
            self.on_hover(now, over);
            return None;
        }
        match self.phase {
            Phase::Parked => self.parked(input),
            Phase::Revealing => self.revealing(now, input),
            Phase::Shown => self.shown(now, input),
            Phase::Retracting => self.retracting(now, input),
        }
    }

    fn parked(&mut self, input: Input) -> Option<Effect> {
        match input {
            Input::Reveal { retract_after } => {
                self.phase = Phase::Revealing;
                self.pending = retract_after;
                Some(Effect::SlideIn)
            }
            _ => None,
        }
    }

    fn revealing(&mut self, now: Instant, input: Input) -> Option<Effect> {
        match input {
            Input::Reveal { retract_after } => {
                self.pending = retract_after;
                None
            }
            Input::ScheduleRetract(after) => {
                self.pending = Some(after);
                None
            }
            Input::KeepShown => {
                self.pending = None;
                None
            }
            Input::Retract => Some(self.start_retract()),
            Input::SlideDone => {
                self.phase = Phase::Shown;
                let pending = self.pending.take();
                self.deadline = self.arm(now, pending);
                None
            }
            Input::Tick | Input::Hover(_) => None,
        }
    }

    fn shown(&mut self, now: Instant, input: Input) -> Option<Effect> {
        match input {
            Input::Reveal { retract_after } => {
                self.deadline = self.arm(now, retract_after);
                None
            }
            Input::ScheduleRetract(after) => {
                self.deadline = self.arm(now, Some(after));
                None
            }
            Input::KeepShown => {
                self.deadline = Deadline::None;
                None
            }
            Input::Retract => Some(self.start_retract()),
            Input::Tick => match self.deadline {
                Deadline::At(t) if now >= t => Some(self.start_retract()),
                _ => None,
            },
            Input::SlideDone | Input::Hover(_) => None,
        }
    }

    fn retracting(&mut self, now: Instant, input: Input) -> Option<Effect> {
        match input {
            Input::Reveal { retract_after } => {
                self.phase = Phase::Revealing;
                self.pending = retract_after;
                Some(Effect::SlideIn)
            }
            Input::SlideDone => {
                self.phase = Phase::Parked;
                self.parked_at = Some(now);
                Some(Effect::Hide)
            }
            Input::Retract
            | Input::Tick
            | Input::ScheduleRetract(_)
            | Input::KeepShown
            | Input::Hover(_) => None,
        }
    }

    fn start_retract(&mut self) -> Effect {
        self.phase = Phase::Retracting;
        self.deadline = Deadline::None;
        self.pending = None;
        Effect::SlideOut
    }

    /// A deadline `after` from now, paused if the pointer is on the head.
    fn arm(&self, now: Instant, after: Option<Duration>) -> Deadline {
        match after {
            None => Deadline::None,
            Some(d) if self.hovered => Deadline::Paused(d),
            Some(d) => Deadline::At(now + d),
        }
    }

    /// Hover pauses auto-retract; it resumes [`HOVER_RESUME`] after the
    /// pointer leaves, with the time that was left.
    fn on_hover(&mut self, now: Instant, over: bool) {
        if over == self.hovered {
            return;
        }
        self.hovered = over;
        self.deadline = match (over, self.deadline) {
            (true, Deadline::At(t)) => Deadline::Paused(t.saturating_duration_since(now)),
            (false, Deadline::Paused(left)) => Deadline::At(now + HOVER_RESUME + left),
            (_, d) => d,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;
    const REVEAL_8S: Input = Input::Reveal {
        retract_after: Some(Duration::from_secs(8)),
    };
    const REVEAL_FOREVER: Input = Input::Reveal {
        retract_after: None,
    };

    /// A machine driven to `phase` at `t0`.
    fn at(phase: Phase, t0: Instant) -> Visibility {
        let mut v = Visibility::new();
        if phase == Phase::Parked {
            return v;
        }
        v.step(t0, REVEAL_8S);
        if phase == Phase::Revealing {
            return v;
        }
        v.step(t0, Input::SlideDone);
        if phase == Phase::Shown {
            return v;
        }
        v.step(t0, Input::Retract);
        v
    }

    /// Every (phase, input) pair: the resulting phase and effect.
    #[test]
    fn transition_table() {
        use Effect::{Hide, SlideIn, SlideOut};
        use Phase::{Parked, Retracting, Revealing, Shown};
        let t0 = Instant::now();
        let cases: &[(Phase, Input, Phase, Option<Effect>)] = &[
            (Parked, REVEAL_8S, Revealing, Some(SlideIn)),
            (Parked, Input::Retract, Parked, None),
            (Parked, Input::SlideDone, Parked, None),
            (Parked, Input::Tick, Parked, None),
            (Parked, Input::ScheduleRetract(MS(1)), Parked, None),
            (Parked, Input::KeepShown, Parked, None),
            (Parked, Input::Hover(true), Parked, None),
            (Revealing, REVEAL_8S, Revealing, None),
            (Revealing, Input::Retract, Retracting, Some(SlideOut)),
            (Revealing, Input::SlideDone, Shown, None),
            (Revealing, Input::Tick, Revealing, None),
            (Revealing, Input::ScheduleRetract(MS(1)), Revealing, None),
            (Revealing, Input::KeepShown, Revealing, None),
            (Revealing, Input::Hover(true), Revealing, None),
            (Shown, REVEAL_8S, Shown, None),
            (Shown, Input::Retract, Retracting, Some(SlideOut)),
            (Shown, Input::SlideDone, Shown, None),
            (Shown, Input::Tick, Shown, None),
            (Shown, Input::ScheduleRetract(MS(1)), Shown, None),
            (Shown, Input::KeepShown, Shown, None),
            (Shown, Input::Hover(true), Shown, None),
            (Retracting, REVEAL_8S, Revealing, Some(SlideIn)),
            (Retracting, Input::Retract, Retracting, None),
            (Retracting, Input::SlideDone, Parked, Some(Hide)),
            (Retracting, Input::Tick, Retracting, None),
            (Retracting, Input::ScheduleRetract(MS(1)), Retracting, None),
            (Retracting, Input::KeepShown, Retracting, None),
            (Retracting, Input::Hover(true), Retracting, None),
        ];
        for &(from, input, to, effect) in cases {
            let mut v = at(from, t0);
            assert_eq!(v.phase(), from);
            assert_eq!(v.step(t0, input), effect, "{from:?} + {input:?}");
            assert_eq!(v.phase(), to, "{from:?} + {input:?}");
        }
    }

    #[test]
    fn auto_retract_counts_from_fully_shown() {
        let t0 = Instant::now();
        let mut v = Visibility::new();
        v.step(t0, REVEAL_8S);
        v.step(t0 + MS(260), Input::SlideDone);
        assert_eq!(v.next_deadline(), Some(t0 + MS(8260)));
        assert_eq!(v.step(t0 + MS(8259), Input::Tick), None);
        assert_eq!(v.step(t0 + MS(8260), Input::Tick), Some(Effect::SlideOut));
    }

    #[test]
    fn reveal_without_delay_stays_shown() {
        let t0 = Instant::now();
        let mut v = Visibility::new();
        v.step(t0, REVEAL_FOREVER);
        v.step(t0, Input::SlideDone);
        assert_eq!(v.next_deadline(), None);
        assert_eq!(v.step(t0 + Duration::from_secs(3600), Input::Tick), None);
    }

    #[test]
    fn wake_while_shown_extends_the_deadline() {
        let t0 = Instant::now();
        let mut v = at(Phase::Shown, t0);
        v.step(t0 + MS(7000), REVEAL_8S);
        assert_eq!(v.next_deadline(), Some(t0 + MS(15_000)));
    }

    #[test]
    fn schedule_and_keep_shown_while_revealing_apply_once_shown() {
        let t0 = Instant::now();
        let mut v = Visibility::new();
        v.step(t0, REVEAL_8S);
        v.step(t0 + MS(10), Input::ScheduleRetract(MS(2500)));
        v.step(t0 + MS(100), Input::SlideDone);
        assert_eq!(v.next_deadline(), Some(t0 + MS(2600)));

        let mut v = Visibility::new();
        v.step(t0, REVEAL_8S);
        v.step(t0, Input::KeepShown);
        v.step(t0, Input::SlideDone);
        assert_eq!(v.next_deadline(), None);
    }

    #[test]
    fn hover_pauses_then_resumes_one_second_after_leaving() {
        let t0 = Instant::now();
        let mut v = at(Phase::Shown, t0); // deadline t0 + 8 s
        v.step(t0 + MS(5000), Input::Hover(true)); // 3 s left
        assert_eq!(v.next_deadline(), None);
        assert_eq!(v.step(t0 + MS(60_000), Input::Tick), None, "paused");
        v.step(t0 + MS(60_000), Input::Hover(false));
        assert_eq!(v.next_deadline(), Some(t0 + MS(64_000)));
    }

    #[test]
    fn hovered_when_reveal_finishes_starts_paused() {
        let t0 = Instant::now();
        let mut v = Visibility::new();
        v.step(t0, REVEAL_8S);
        v.step(t0, Input::Hover(true));
        v.step(t0 + MS(260), Input::SlideDone);
        assert_eq!(v.next_deadline(), None);
        v.step(t0 + MS(500), Input::Hover(false));
        assert_eq!(
            v.next_deadline(),
            Some(t0 + MS(500) + HOVER_RESUME + MS(8000))
        );
    }

    #[test]
    fn retract_mid_reveal_and_reveal_mid_retract_reverse() {
        let t0 = Instant::now();
        let mut v = Visibility::new();
        assert_eq!(v.step(t0, REVEAL_8S), Some(Effect::SlideIn));
        assert_eq!(v.step(t0 + MS(100), Input::Retract), Some(Effect::SlideOut));
        // A stale "reveal done" can't arrive: the caller restarted the slide.
        assert_eq!(v.step(t0 + MS(150), REVEAL_8S), Some(Effect::SlideIn));
        assert_eq!(v.phase(), Phase::Revealing);
        v.step(t0 + MS(300), Input::SlideDone);
        assert_eq!(v.phase(), Phase::Shown);
    }

    #[test]
    fn retract_clears_deadline_and_pending() {
        let t0 = Instant::now();
        let mut v = at(Phase::Shown, t0);
        v.step(t0, Input::Retract);
        assert_eq!(v.next_deadline(), None);
        // Reveal again with no delay: the old 8 s must not come back.
        v.step(t0, REVEAL_FOREVER);
        v.step(t0, Input::SlideDone);
        assert_eq!(v.next_deadline(), None);
    }

    #[test]
    fn corner_wake_cooldown_after_retract() {
        let t0 = Instant::now();
        let cooldown = MS(1500);
        let mut v = Visibility::new();
        assert!(v.corner_wake_allowed(t0, cooldown), "never retracted");
        v.step(t0, REVEAL_8S);
        assert!(!v.corner_wake_allowed(t0, cooldown), "not parked");
        v.step(t0, Input::SlideDone);
        v.step(t0, Input::Retract);
        v.step(t0 + MS(200), Input::SlideDone);
        assert!(!v.corner_wake_allowed(t0 + MS(1699), cooldown));
        assert!(v.corner_wake_allowed(t0 + MS(1700), cooldown));
    }
}
