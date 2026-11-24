//! Reminders (spec §5.10): waits for the next one and announces it.

use std::time::Duration;

use sohi_audio::sound::Sound;
use sohi_core::interaction::Event;
use sohi_core::reminder::announce;
use sohi_core::stamp::Stamp;
use sohi_tools::Agenda;
use sohi_win::AppTimer;

use super::{TIMER_REMINDER, TextMode};
use crate::chat::{Request, SayTurn};
use crate::full_mode::FullMode;
use crate::head::Head;

/// The longest wait. Short enough to notice soon that the clock was set.
const LONGEST: Duration = Duration::from_mins(10);
/// The wait while Sohi is busy with something else.
const BUSY: Duration = Duration::from_secs(2);
/// What is missed is announced this long after starting up.
const AT_START: Duration = Duration::from_secs(4);
/// What the models read before an announcement, so a follow-up ("again in
/// ten minutes") has something to refer to.
const CAME_DUE: &str = "(A reminder came due.)";

/// What there is to announce, and when.
pub(super) struct Reminding {
    agenda: Agenda,
    speak: bool,
    timer: Option<AppTimer>,
    /// When the reminder is due that `timer` is for.
    armed: Option<Stamp>,
    /// The session is locked, or the PC asleep.
    away: bool,
    /// For the toast, which is not text mode's to show.
    toast: Option<String>,
}

impl Reminding {
    pub(super) fn new(agenda: Agenda, speak: bool) -> Self {
        Self {
            agenda,
            speak,
            timer: None,
            armed: None,
            away: false,
            toast: None,
        }
    }

    pub(super) fn agenda(&self) -> Agenda {
        self.agenda.clone()
    }
}

impl TextMode {
    /// Starts waiting for the first one.
    pub(super) fn start_reminders(&mut self) {
        self.wait(AT_START);
    }

    /// The reminders may have changed: waits for the next one.
    pub fn arm_reminders(&mut self) {
        let reminding = &self.reminding;
        if reminding.timer.is_some() && reminding.armed == reminding.agenda.next_at() {
            return;
        }
        self.wait(Duration::from_millis(50));
    }

    /// Waits for the next reminder, at least `least`.
    fn wait(&mut self, least: Duration) {
        let next = self.reminding.agenda.next_at();
        self.reminding.armed = next;
        // Dropping it after the new one was started would stop that: they
        // share an id.
        self.reminding.timer = None;
        self.reminding.timer = next.filter(|_| !self.reminding.away).and_then(|at| {
            let seconds = at.since(sohi_win::local_stamp());
            let wait = Duration::from_secs(u64::try_from(seconds).unwrap_or(0));
            self.start_timer(TIMER_REMINDER, wait.clamp(least, LONGEST))
        });
    }

    /// Whether the session is locked or the PC asleep: then nothing is
    /// announced.
    pub fn set_away(&mut self, away: bool, head: &mut Head) {
        if std::mem::replace(&mut self.reminding.away, away) == away {
            return;
        }
        if away {
            self.reminding.timer = None;
        } else {
            self.check_reminders(head);
        }
    }

    /// Announces what is due, if this is a good moment.
    pub(super) fn check_reminders(&mut self, head: &mut Head) {
        // It goes on ticking until dropped.
        self.reminding.timer = None;
        let now = sohi_win::local_stamp();
        let due = self.reminding.agenda.next_at().is_some_and(|at| at <= now);
        if !due || self.reminding.away {
            return self.arm_reminders();
        }
        let moving = self.full.as_ref().is_some_and(FullMode::is_playing);
        if moving || !self.interaction.can_announce() {
            return self.wait(BUSY);
        }
        let due = self.reminding.agenda.take_due(now);
        let text = announce(&due, now);
        // Full mode itself is full screen, too, even behind a window.
        let ours = self.is_full();
        let quiet = !ours && sohi_win::shell::do_not_disturb().is_some();
        let aloud = self.reminding.speak && !quiet;
        if !quiet {
            self.sounds.play(Sound::Reminder);
        }
        log::info!("{} reminder(s) due (aloud: {aloud})", due.len());
        self.history.push_user(CAME_DUE);
        self.history.push_reply(text.clone(), Vec::new());
        self.reminding.toast = Some(text.clone());
        self.event(Event::Announce { text, aloud }, head);
        self.arm_reminders();
    }

    /// What a toast is to say now, if anything.
    pub fn take_toast(&mut self) -> Option<String> {
        self.reminding.toast.take()
    }

    /// Says `text`. Returns a follow-up event if that is not possible.
    pub(super) fn say(&mut self, turn: u64, text: String) -> Option<Event> {
        let mut tts = self.settings.tts.clone();
        if !self.net.state().is_usable() {
            // The Windows voice.
            tts.provider.clear();
        }
        let request = Request::Say(Box::new(SayTurn { turn, text, tts }));
        let sent = self.runtime().map(|r| r.send(request));
        if matches!(sent, Some(Ok(()))) {
            return None;
        }
        log::error!("cannot say notice {turn}: {sent:?}");
        Some(Event::PlaybackDone { turn })
    }

    /// Lists them in full mode.
    pub(super) fn list_reminders(&mut self) {
        let mut lines = self.reminding.agenda.lines();
        if lines.is_empty() {
            lines.push("No reminders. Ask for one: “remind me at 5 to call Mum”.".to_owned());
        } else {
            lines.insert(0, "Reminders".to_owned());
        }
        if let Some(full) = self.full.as_mut() {
            full.begin(&lines.join("\n"));
        }
    }
}
