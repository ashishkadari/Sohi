//! Connectivity (spec §5.3): carries the region's effects out, and lets it
//! decide what a wake does and how the idle head looks.

use std::time::{Duration, Instant};

use sohi_core::connectivity::{Effect, Event, State};
use sohi_core::interaction::{Event as Turn, State as Talk};
use sohi_core::visual::VisualState;
use sohi_providers::ProviderError;

use super::{TIMER_BACKOFF, TextMode};
use crate::chat::{Request, Update};
use crate::head::Head;

/// A wake that found Sohi offline becomes a voice wake if a probe succeeds
/// this soon.
const UPGRADE: Duration = Duration::from_millis(400);

impl TextMode {
    /// A voice wake. Offline, it opens the pill instead; `focus` says
    /// whether the pill may then take the keyboard.
    pub fn wake(&mut self, focus: bool, head: &mut Head) {
        self.front_full();
        let starts = matches!(self.interaction.state(), Talk::Idle);
        self.net(Event::Wake, head);
        self.offline_wake = (starts && self.net.state() == State::Offline).then(Instant::now);
        self.event(Turn::VoiceWake { focus }, head);
    }

    /// The text hotkey, the tray's Type, or a click on the head.
    pub fn wake_text(&mut self, event: Turn, head: &mut Head) {
        self.net(Event::Wake, head);
        if self.front_full() {
            return;
        }
        self.event(event, head);
    }

    /// Toggles forced offline; returns whether it is now on. A running
    /// turn is cancelled: nothing may be sent any more.
    pub fn toggle_offline(&mut self, head: &mut Head) -> bool {
        self.net(Event::ForceToggled, head);
        let forced = self.net.state() == State::ForcedOffline;
        if forced && self.interaction.current_turn().is_some() {
            self.event(Turn::Escape, head);
        }
        forced
    }

    /// Feeds a Connectivity event and carries out the effects.
    pub fn net(&mut self, event: Event, head: &mut Head) {
        for effect in self.net.step(event) {
            match effect {
                Effect::StartProbe { id } => {
                    log::info!("probe {id}");
                    if !matches!(
                        self.runtime().map(|r| r.send(Request::Probe { id })),
                        Some(Ok(()))
                    ) {
                        log::warn!("cannot send probe {id}");
                        self.net(Event::ProbeFailed { id }, head);
                    }
                }
                Effect::StartBackoff(wait) => {
                    log::info!("offline: next probe in {} s", wait.as_secs());
                    self.backoff = self.start_timer(TIMER_BACKOFF, wait);
                }
                Effect::StopBackoff => self.backoff = None,
                Effect::Changed => self.connectivity_changed(head),
            }
        }
    }

    /// Takes what an update says about connectivity.
    pub(super) fn learn(&mut self, update: &Update, head: &mut Head) {
        let event = match update {
            Update::Probed { id, reached: true } => Event::ProbeOk { id: *id },
            Update::Probed { id, reached: false } => Event::ProbeFailed { id: *id },
            Update::Transcript { .. } | Update::Command { .. } => Event::RequestOk,
            Update::Failed {
                error: ProviderError::Offline(_),
                ..
            } => Event::RequestFailed,
            _ => return,
        };
        self.net(event, head);
    }

    /// Follows the head in and out of the parked position.
    pub(super) fn net_shown(&mut self, shown: bool, head: &mut Head) {
        self.net(Event::Shown(shown), head);
    }

    fn connectivity_changed(&mut self, head: &mut Head) {
        let state = self.net.state();
        log::info!("connectivity: {state:?}");
        self.interaction.set_voice_available(state.is_usable());
        self.with_pill(|pill| pill.set_placeholder(&self.placeholder(false)));
        self.show_base(head);
        self.settle_full(head);
        let woken = self.offline_wake;
        if !state.is_usable() {
            return;
        }
        self.offline_wake = None;
        if woken.is_some_and(|at| at.elapsed() <= UPGRADE)
            && self.interaction.state() == Talk::TextEntry
            && self.pill.as_ref().is_none_or(|p| !p.has_input())
        {
            log::info!("online after all: listening");
            self.event(Turn::VoiceWake { focus: false }, head);
        }
    }

    /// Shows the idle look that fits the connectivity.
    pub fn show_base(&self, head: &mut Head) {
        if is_base(head.visual()) {
            head.set_visual(self.base_visual());
        }
    }

    /// What the head shows when nothing is going on.
    pub(super) fn base_visual(&self) -> VisualState {
        match self.net.state() {
            State::Online | State::Probing => VisualState::Idle,
            State::Offline => VisualState::Offline,
            State::ForcedOffline => VisualState::ForcedOffline,
        }
    }

    /// The pill's placeholder, `focused` or not.
    pub(super) fn placeholder(&self, focused: bool) -> String {
        if self.net.state().is_usable() {
            "Type a message…".to_owned()
        } else if focused || self.settings.hotkey_text.is_empty() {
            "Offline. Type a message…".to_owned()
        } else {
            format!(
                "Offline. Click or press {} to type.",
                self.settings.hotkey_text
            )
        }
    }
}

fn is_base(visual: VisualState) -> bool {
    matches!(
        visual,
        VisualState::Idle | VisualState::Offline | VisualState::ForcedOffline
    )
}
