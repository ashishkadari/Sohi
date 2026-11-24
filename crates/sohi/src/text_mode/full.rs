//! Full mode (spec §4.9): in and out of it, what its window asks for, and
//! the turns' effects while it is open. The pill and the head are away
//! then.

use std::rc::Rc;
use std::time::Duration;

use sohi_core::chat::Role;
use sohi_core::connectivity::State as Net;
use sohi_core::full::{Command, command};
use sohi_core::history::Reach;
use sohi_core::interaction::{Effect, Event, State};
use sohi_win::FullEvent;

use super::{READ_FIRST, TextMode};
use crate::full_mode::{FullMode, Step};
use crate::head::Head;

/// How long the head stays after full mode was left.
const AFTER: Duration = Duration::from_millis(1500);

impl TextMode {
    /// Opens full mode, or brings it to the front.
    pub fn enter_full(&mut self, head: &mut Head) {
        if self.front_full() {
            return;
        }
        let monitor = match sohi_win::primary_monitor() {
            Ok(monitor) => monitor,
            Err(err) => return log::warn!("cannot read primary monitor: {err}"),
        };
        let animated = !head.reduced_motion();
        let ticker = Rc::clone(&self.ticker);
        match FullMode::open(&self.queue, ticker, monitor, self.head_rect, animated) {
            Ok(full) => self.full = Some(full),
            Err(err) => return log::error!("cannot open full mode: {err}"),
        }
        log::info!("full mode: open (animated: {animated})");
        self.hide_pill();
        head.vanish();
        self.show_history();
        // The machine waits for text while full mode is open.
        self.settle_full(head);
        if let Some(full) = &self.full
            && !full.is_playing()
        {
            full.arrive();
        }
    }

    /// Starts leaving full mode. A running turn is cancelled.
    pub fn leave_full(&mut self, head: &mut Head) {
        if self.full.as_ref().is_none_or(FullMode::is_leaving) {
            return;
        }
        if self.interaction.current_turn().is_some() {
            self.event(Event::Escape, head);
        }
        let animated = !head.reduced_motion();
        if self.full.as_mut().is_some_and(|f| !f.leave(animated)) {
            self.close_full(head);
        }
    }

    /// Toggles full mode (tray menu).
    pub fn toggle_full(&mut self, head: &mut Head) {
        if self.full.is_some() {
            self.leave_full(head);
        } else {
            self.enter_full(head);
        }
    }

    /// Whether full mode is open.
    pub fn is_full(&self) -> bool {
        self.full.is_some()
    }

    fn close_full(&mut self, head: &mut Head) {
        head.appear(AFTER);
        self.full = None;
        log::info!("full mode: closed");
        self.event(Event::HeadGone, head);
    }

    /// Brings full mode to the front. Returns `false` if it is not open.
    pub(super) fn front_full(&self) -> bool {
        let Some(full) = &self.full else {
            return false;
        };
        if !full.is_front() && !full.is_playing() {
            full.arrive();
        }
        true
    }

    /// The session locked, or the display went off: nobody is there.
    pub fn away(&mut self, head: &mut Head) {
        if self.is_full() && self.interaction.current_turn().is_some() {
            self.event(Event::Escape, head);
        }
    }

    /// Draws a frame.
    pub fn full_frame(&mut self, head: &mut Head) {
        let Some(full) = self.full.as_mut() else {
            return;
        };
        match full.on_frame(&head.scene()) {
            Step::Going => {}
            Step::Arrived => {
                full.arrive();
            }
            Step::Left => self.close_full(head),
        }
    }

    /// Handles the window's events.
    pub fn on_full(&mut self, event: FullEvent, head: &mut Head) {
        if self.full.as_ref().is_none_or(FullMode::is_leaving) {
            return;
        }
        let busy = self.interaction.current_turn().is_some();
        match event {
            FullEvent::Submit => self.submit_full(head),
            FullEvent::Escape | FullEvent::Talk if busy => self.event(Event::Escape, head),
            FullEvent::Talk => self.wake(true, head),
            FullEvent::Escape | FullEvent::Exit => self.leave_full(head),
            FullEvent::CopyCode => self.copy_code(),
            FullEvent::NewChat => {
                if busy {
                    self.event(Event::Escape, head);
                }
                self.history.clear();
                if let Some(full) = self.full.as_mut() {
                    full.start_over();
                }
            }
        }
    }

    /// Enter. Text typed while a reply is being written stays.
    fn submit_full(&mut self, head: &mut Head) {
        match self.interaction.state() {
            State::Confirming { .. } if self.asked.elapsed() < READ_FIRST => return,
            State::Thinking { .. } | State::Transcribing { .. } | State::Error => return,
            // Typing instead of talking.
            State::Listening { .. } => self.event(Event::Escape, head),
            _ => {}
        }
        let Some(full) = &self.full else {
            return;
        };
        let text = full.window().take_input();
        match command(&text) {
            Some(Command::Leave) => self.leave_full(head),
            Some(Command::Enter) => {}
            None => self.event(Event::TextSubmitted(text), head),
        }
    }

    fn copy_code(&self) {
        let Some(full) = &self.full else {
            return;
        };
        let said = match full.code().map(sohi_win::set_clipboard_text) {
            Some(Ok(())) => "Code copied.",
            Some(Err(err)) => {
                log::warn!("cannot copy the code: {err}");
                "The clipboard is busy. Try again."
            }
            None => "There is no code to copy yet.",
        };
        full.window().set_status(said);
    }

    /// Carries out a spoken command. Its turn is over.
    pub(super) fn obey(&mut self, command: Command, head: &mut Head) {
        log::info!("full mode: {command:?}");
        match command {
            Command::Enter => {
                self.enter_full(head);
                self.event(Event::Escape, head);
            }
            Command::Leave => {
                self.event(Event::Escape, head);
                self.leave_full(head);
            }
        }
    }

    /// Takes the effects that show things, while full mode is open.
    /// Returns the ones it leaves to the usual handling.
    pub(super) fn full_effect(&mut self, effect: Effect) -> Option<Effect> {
        let Some(full) = self.full.as_mut() else {
            return Some(effect);
        };
        match effect {
            Effect::SetBubble(text) => full.begin(&text),
            Effect::AppendBubble(text) => full.stream(&text),
            Effect::Reveal(_)
            | Effect::Keep(_)
            | Effect::Retract
            | Effect::ShowPill { .. }
            | Effect::HidePill => {}
            other => return Some(other),
        }
        None
    }

    /// After an event: full mode always waits for text when idle, and says
    /// what is going on.
    pub(super) fn settle_full(&mut self, head: &mut Head) {
        if self.full.is_none() {
            return;
        }
        if self.interaction.state() == State::Idle {
            self.event(Event::HeadClicked, head);
            return;
        }
        let status = self.full_status();
        let busy = self.interaction.current_turn().is_some();
        if let Some(full) = self.full.as_mut() {
            if !busy {
                full.end_reply();
            }
            full.window().set_status(status);
            full.window()
                .set_talk_label(if busy { "Stop" } else { "Talk" });
        }
    }

    fn full_status(&self) -> &'static str {
        match self.interaction.state() {
            State::Idle | State::TextEntry => match self.net.state() {
                Net::Online | Net::Probing => "Ready",
                Net::Offline => "Offline",
                Net::ForcedOffline => "Offline (forced)",
            },
            State::Listening { .. } => "Listening…",
            State::Transcribing { .. } => "Transcribing…",
            State::Thinking { .. } => "Thinking…",
            State::Speaking { .. } => "Speaking…",
            State::Confirming { .. } => "Waiting for you: Enter = Yes, Esc = No",
            State::Error => "Something went wrong",
        }
    }

    /// Shows the conversation so far.
    fn show_history(&mut self) {
        let Some(full) = self.full.as_mut() else {
            return;
        };
        full.start_over();
        for message in self.history.messages(Reach::Long) {
            match message.role {
                Role::User => full.begin(&format!("› {}", message.content)),
                Role::Assistant if !message.content.is_empty() => full.stream(&message.content),
                Role::Assistant | Role::Tool => {}
            }
        }
    }

    /// Ends the reply on screen: what follows is something else.
    pub(super) fn end_full_reply(&mut self) {
        if let Some(full) = self.full.as_mut() {
            full.end_reply();
        }
    }
}
