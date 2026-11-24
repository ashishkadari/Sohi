//! The Connectivity region of the state machine (spec §5.3).
//!
//! Pure: events in, effects out. Network hints are advisory; only a probe
//! or a real request decides. While the head is parked nothing is polled:
//! a probe then follows only a hint or a wake, and no timer runs.

use std::time::Duration;

/// Waits between probes while offline and shown; the last one repeats.
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_mins(1),
];
/// This many failed probes in a row mean offline.
const FAILURES: u8 = 2;

/// Whether the online services can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// As far as known.
    Online,
    /// Something failed; a probe is finding out.
    Probing,
    /// Probes failed.
    Offline,
    /// The user said so; nothing but the user changes it.
    ForcedOffline,
}

impl State {
    /// Whether online turns may be tried.
    #[must_use]
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Online | Self::Probing)
    }
}

/// Something that bears on connectivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A request could not reach its service (never an HTTP status).
    RequestFailed,
    /// A request was answered.
    RequestOk,
    /// Windows reports whether there is internet access.
    Hint {
        /// Whether there is.
        internet: bool,
    },
    /// Probe `id` got any HTTP answer.
    ProbeOk {
        /// The probe.
        id: u64,
    },
    /// Probe `id` got none.
    ProbeFailed {
        /// The probe.
        id: u64,
    },
    /// The user woke Sohi.
    Wake,
    /// The head left the parked position (`true`) or returned to it.
    Shown(bool),
    /// The backoff timer fired.
    BackoffElapsed,
    /// The system resumed from sleep.
    Resumed,
    /// The user toggled "Force offline".
    ForceToggled,
}

/// What to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Send one probe and report back with its id.
    StartProbe {
        /// The probe.
        id: u64,
    },
    /// Send [`Event::BackoffElapsed`] after this long.
    StartBackoff(Duration),
    /// Stop the backoff timer.
    StopBackoff,
    /// [`Connectivity::state`] changed.
    Changed,
}

/// The Connectivity region.
#[derive(Debug, Clone)]
pub struct Connectivity {
    state: State,
    /// Failed probes in a row.
    failures: u8,
    /// The probe in flight.
    probe: Option<u64>,
    next_probe: u64,
    /// A hint arrived during the probe: its failure may be out of date.
    again: bool,
    shown: bool,
    /// Index into [`BACKOFF`].
    backoff: usize,
    /// The system slept: check at the next wake.
    stale: bool,
}

impl Connectivity {
    /// Starts online (the first request finds out), or forced offline.
    #[must_use]
    pub fn new(forced: bool) -> Self {
        Self {
            state: if forced {
                State::ForcedOffline
            } else {
                State::Online
            },
            failures: 0,
            probe: None,
            next_probe: 1,
            again: false,
            shown: false,
            backoff: 0,
            stale: false,
        }
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Feeds one event and returns the effects, in order.
    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Shown(shown) => return self.show(shown),
            Event::ForceToggled => return self.toggle_force(),
            Event::Resumed => {
                self.stale = true;
                return Vec::new();
            }
            _ => {}
        }
        match (self.state, event) {
            (State::Online, Event::RequestFailed | Event::Hint { internet: false }) => self.doubt(),
            (State::Online, Event::Wake) if self.stale => self.doubt(),
            (State::Probing | State::Offline, Event::Hint { .. }) => {
                self.again = self.probe.is_some();
                self.start_probe()
            }
            (State::Offline, Event::Wake) => self.start_probe(),
            (State::Offline, Event::BackoffElapsed) if self.shown => self.start_probe(),
            (State::Probing | State::Offline, Event::RequestOk) => self.online(),
            (State::Probing | State::Offline, Event::ProbeOk { id }) if self.probe == Some(id) => {
                self.online()
            }
            (State::Probing | State::Offline, Event::ProbeFailed { id })
                if self.probe == Some(id) =>
            {
                self.probe_failed()
            }
            _ => Vec::new(),
        }
    }

    /// Online may be over: find out.
    fn doubt(&mut self) -> Vec<Effect> {
        self.state = State::Probing;
        self.failures = 0;
        self.stale = false;
        let mut effects = vec![Effect::Changed];
        effects.extend(self.start_probe());
        effects
    }

    fn online(&mut self) -> Vec<Effect> {
        self.state = State::Online;
        self.probe = None;
        self.again = false;
        self.stale = false;
        self.backoff = 0;
        vec![Effect::StopBackoff, Effect::Changed]
    }

    fn probe_failed(&mut self) -> Vec<Effect> {
        self.probe = None;
        if std::mem::take(&mut self.again) {
            self.failures = 0;
            return self.start_probe();
        }
        self.failures += 1;
        if self.state == State::Offline {
            return self.back_off();
        }
        if self.failures < FAILURES {
            return self.start_probe();
        }
        self.state = State::Offline;
        let mut effects = vec![Effect::Changed];
        effects.extend(self.back_off());
        effects
    }

    /// Waits before the next probe, but only while shown.
    fn back_off(&mut self) -> Vec<Effect> {
        if !self.shown {
            return Vec::new();
        }
        let wait = BACKOFF[self.backoff.min(BACKOFF.len() - 1)];
        self.backoff = (self.backoff + 1).min(BACKOFF.len() - 1);
        vec![Effect::StartBackoff(wait)]
    }

    fn show(&mut self, shown: bool) -> Vec<Effect> {
        if self.shown == shown {
            return Vec::new();
        }
        self.shown = shown;
        self.backoff = 0;
        match (self.state, shown) {
            (State::Offline, true) if self.probe.is_none() => self.back_off(),
            (State::Offline, false) => vec![Effect::StopBackoff],
            _ => Vec::new(),
        }
    }

    fn toggle_force(&mut self) -> Vec<Effect> {
        if self.state == State::ForcedOffline {
            return self.doubt();
        }
        self.state = State::ForcedOffline;
        // A probe still in flight no longer counts.
        self.probe = None;
        self.again = false;
        vec![Effect::StopBackoff, Effect::Changed]
    }

    /// Probes, unless one is already in flight.
    fn start_probe(&mut self) -> Vec<Effect> {
        if self.probe.is_some() {
            return Vec::new();
        }
        let id = self.next_probe;
        self.next_probe += 1;
        self.probe = Some(id);
        vec![Effect::StartProbe { id }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_INTERNET: Event = Event::Hint { internet: false };
    const INTERNET: Event = Event::Hint { internet: true };

    /// Offline after two failed probes, parked.
    fn offline() -> Connectivity {
        let mut c = Connectivity::new(false);
        assert_eq!(
            c.step(NO_INTERNET),
            [Effect::Changed, Effect::StartProbe { id: 1 }]
        );
        assert_eq!(c.state(), State::Probing);
        assert_eq!(
            c.step(Event::ProbeFailed { id: 1 }),
            [Effect::StartProbe { id: 2 }]
        );
        assert_eq!(c.step(Event::ProbeFailed { id: 2 }), [Effect::Changed]);
        assert_eq!(c.state(), State::Offline);
        c
    }

    fn started(effects: &[Effect]) -> Option<u64> {
        effects.iter().find_map(|e| match e {
            Effect::StartProbe { id } => Some(*id),
            _ => None,
        })
    }

    #[test]
    fn a_failed_request_is_checked_by_a_probe() {
        let mut c = Connectivity::new(false);
        let effects = c.step(Event::RequestFailed);
        assert_eq!(started(&effects), Some(1));
        assert!(c.state().is_usable());
        assert_eq!(
            c.step(Event::ProbeOk { id: 1 }),
            [Effect::StopBackoff, Effect::Changed]
        );
        assert_eq!(c.state(), State::Online);
    }

    #[test]
    fn a_request_that_works_settles_it() {
        let mut c = Connectivity::new(false);
        c.step(Event::RequestFailed);
        c.step(Event::RequestOk);
        assert_eq!(c.state(), State::Online);
        assert_eq!(c.step(Event::ProbeFailed { id: 1 }), [], "out of date");
        assert_eq!(c.state(), State::Online);
    }

    #[test]
    fn good_hints_mean_nothing_while_online() {
        let mut c = Connectivity::new(false);
        assert_eq!(c.step(INTERNET), []);
        assert_eq!(c.step(Event::Wake), []);
    }

    #[test]
    fn parked_and_offline_nothing_runs() {
        let mut c = offline();
        assert_eq!(c.step(Event::BackoffElapsed), []);
        // A wake or a hint probes once; its failure starts no timer.
        for event in [Event::Wake, INTERNET, NO_INTERNET] {
            let id = started(&c.step(event)).unwrap();
            assert_eq!(c.step(Event::ProbeFailed { id }), [], "{event:?}");
        }
        assert_eq!(c.state(), State::Offline);
    }

    #[test]
    fn shown_and_offline_probes_back_off() {
        let mut c = offline();
        let mut waits = vec![c.step(Event::Shown(true))];
        for _ in 0..3 {
            let id = started(&c.step(Event::BackoffElapsed)).unwrap();
            waits.push(c.step(Event::ProbeFailed { id }));
        }
        assert_eq!(
            waits.concat(),
            [15, 30, 60, 60].map(|s| Effect::StartBackoff(Duration::from_secs(s)))
        );
        assert_eq!(c.step(Event::Shown(false)), [Effect::StopBackoff]);
        assert_eq!(c.step(Event::BackoffElapsed), []);
        // Shown again: from the start.
        assert_eq!(
            c.step(Event::Shown(true)),
            [Effect::StartBackoff(Duration::from_secs(15))]
        );
    }

    #[test]
    fn a_wake_probe_brings_it_back() {
        let mut c = offline();
        let id = started(&c.step(Event::Wake)).unwrap();
        assert_eq!(c.step(Event::Wake), [], "one at a time");
        c.step(Event::ProbeOk { id });
        assert_eq!(c.state(), State::Online);
    }

    #[test]
    fn a_hint_during_a_probe_repeats_it() {
        let mut c = offline();
        let id = started(&c.step(Event::Wake)).unwrap();
        assert_eq!(c.step(INTERNET), []);
        let next = started(&c.step(Event::ProbeFailed { id })).unwrap();
        assert_eq!(c.step(Event::ProbeFailed { id: next }), []);
    }

    #[test]
    fn forced_offline_stays_until_toggled() {
        let mut c = Connectivity::new(false);
        c.step(Event::RequestFailed);
        c.step(Event::ForceToggled);
        assert_eq!(c.state(), State::ForcedOffline);
        for event in [
            Event::ProbeOk { id: 1 },
            Event::ProbeFailed { id: 1 },
            Event::RequestOk,
            Event::RequestFailed,
            INTERNET,
            NO_INTERNET,
            Event::Wake,
            Event::Shown(true),
            Event::BackoffElapsed,
            Event::Resumed,
        ] {
            assert_eq!(c.step(event), [], "{event:?}");
            assert_eq!(c.state(), State::ForcedOffline);
        }
        assert!(started(&c.step(Event::ForceToggled)).is_some());
        assert_eq!(c.state(), State::Probing);
        assert_eq!(Connectivity::new(true).state(), State::ForcedOffline);
    }

    #[test]
    fn the_first_wake_after_sleep_checks() {
        let mut c = Connectivity::new(false);
        assert_eq!(c.step(Event::Resumed), []);
        assert!(started(&c.step(Event::Wake)).is_some());
        c.step(Event::ProbeOk { id: 1 });
        assert_eq!(c.step(Event::Wake), [], "only the first");
    }
}
