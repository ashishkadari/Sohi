//! Full mode's windows (spec §4.9): the animation, the live head in the
//! header, and the conversation as it is written. What a turn does is up
//! to [`crate::text_mode`].

use std::rc::Rc;
use std::time::{Duration, Instant};

use sohi_core::full::{Take, layout};
use sohi_core::geometry::RectPx;
use sohi_core::markdown::{Markdown, Span, Style};
use sohi_render::{CapeRenderer, HeadRenderer, Scene};
use sohi_win::{
    EventQueue, FRAME_30_FPS, FRAME_60_FPS, FullWindow, MonitorInfo, Overlay, TextStyle, WinError,
};

use crate::ticker::{Ticker, Who};

/// How often to look again while another application is in front and
/// nothing is going on.
const BEHIND: Duration = Duration::from_millis(500);

/// What a frame brought about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing special.
    Going,
    /// The cape covered the screen on the way in: the window is there.
    Arrived,
    /// The way out is over: full mode can go.
    Left,
}

/// An animation under way.
struct Play {
    take: Take,
    start: Instant,
    overlay: Overlay,
    cape: CapeRenderer,
    /// The window was shown or hidden behind the cape.
    switched: bool,
    /// What the frames cost.
    costs: Vec<Duration>,
}

/// The full mode window and what goes on in it.
pub struct FullMode {
    window: FullWindow,
    ticker: Rc<Ticker>,
    play: Option<Play>,
    avatar: Option<HeadRenderer>,
    reply: Option<Markdown>,
    last_code: Option<String>,
    /// Nothing was said yet.
    empty: bool,
    /// Something is going on: the head moves even behind other windows.
    lively: bool,
    head: RectPx,
    monitor: MonitorInfo,
}

impl FullMode {
    /// Opens full mode over `monitor`; `head` is where the head in the
    /// corner is. With `animated`, the cape goes first.
    ///
    /// # Errors
    /// Returns [`WinError`] if the window cannot be created.
    pub fn open(
        queue: &EventQueue,
        ticker: Rc<Ticker>,
        monitor: MonitorInfo,
        head: RectPx,
        animated: bool,
    ) -> Result<Self, WinError> {
        let window =
            FullWindow::create(queue, monitor.monitor, monitor.dpi, sohi_render::tray_icon)?;
        let mut full = Self {
            window,
            ticker,
            play: None,
            avatar: None,
            reply: None,
            last_code: None,
            empty: true,
            lively: false,
            head,
            monitor,
        };
        full.play = animated.then(|| full.start(Take::Over)).flatten();
        full.reschedule();
        Ok(full)
    }

    /// Whether it opened without an animation: the caller shows it.
    pub fn is_playing(&self) -> bool {
        self.play.is_some()
    }

    /// Shows the window and takes the keyboard. Returns whether it got it.
    pub fn arrive(&self) -> bool {
        self.window.show();
        let focused = self.window.focus();
        if !focused {
            log::info!("full mode: Windows kept the keyboard elsewhere");
        }
        focused
    }

    /// Starts the way out. Returns `false` if there is nothing to play:
    /// full mode can go at once.
    pub fn leave(&mut self, animated: bool) -> bool {
        if self.is_leaving() {
            return true;
        }
        // From wherever the way in had got to.
        self.play = animated.then(|| self.start(Take::Back)).flatten();
        self.reschedule();
        self.play.is_some()
    }

    /// Whether it is on the way out.
    pub fn is_leaving(&self) -> bool {
        self.play.as_ref().is_some_and(|p| p.take == Take::Back)
    }

    /// Whether the window is in front.
    pub fn is_front(&self) -> bool {
        self.window.is_front()
    }

    /// The window, for its input and status.
    pub fn window(&self) -> &FullWindow {
        &self.window
    }

    /// Moves to a changed monitor.
    pub fn set_layout(&mut self, monitor: MonitorInfo, head: RectPx) {
        self.head = head;
        if monitor == self.monitor {
            return;
        }
        self.monitor = monitor;
        // Its size is wrong now.
        let leaving = self.is_leaving();
        if let Some(play) = self.play.take()
            && !leaving
            && !play.switched
        {
            self.arrive();
        }
        if let Err(err) = self.window.set_bounds(monitor.monitor, monitor.dpi) {
            log::warn!("cannot lay out full mode: {err}");
        }
        self.reschedule();
    }

    /// Draws one frame; `scene` is what the head shows now.
    pub fn on_frame(&mut self, scene: &Scene) -> Step {
        let step = self.play_frame();
        let hidden = self.play.as_ref().is_some_and(|p| !p.switched);
        self.lively = scene.state.fps() == Some(60) || scene.error_age.is_some();
        if !hidden && (self.lively || self.window.is_front()) {
            self.draw_avatar(scene);
        }
        self.reschedule();
        step
    }

    fn play_frame(&mut self) -> Step {
        let Some(play) = self.play.as_mut() else {
            return Step::Going;
        };
        let started = Instant::now();
        let at = started.saturating_duration_since(play.start);
        if at >= play.take.duration() {
            return self.finish();
        }
        let frame = play.cape.render(&play.take.moment(at), at);
        if let Err(err) = play.overlay.present(frame) {
            log::warn!("full mode: {err}");
            return self.finish();
        }
        play.costs.push(started.elapsed());
        if play.switched || at < play.take.covered_at() {
            return Step::Going;
        }
        play.switched = true;
        match play.take {
            Take::Over => Step::Arrived,
            Take::Back => {
                self.window.hide();
                Step::Going
            }
        }
    }

    /// Ends the animation, whether or not it got to the switch.
    fn finish(&mut self) -> Step {
        let Some(play) = self.play.take() else {
            return Step::Going;
        };
        report(play.take, play.costs);
        match play.take {
            Take::Over if play.switched => Step::Going,
            Take::Over => Step::Arrived,
            Take::Back => Step::Left,
        }
    }

    fn start(&self, take: Take) -> Option<Play> {
        let overlay = Overlay::create(self.monitor.monitor)
            .inspect_err(|err| log::warn!("full mode: {err}"))
            .ok()?;
        let (width, height) = overlay.frame_size();
        let m = self.monitor.monitor;
        let avatar = layout(m.right - m.left, m.bottom - m.top, self.monitor.dpi).avatar;
        let head = RectPx {
            left: self.head.left - m.left,
            top: self.head.top - m.top,
            right: self.head.right - m.left,
            bottom: self.head.bottom - m.top,
        };
        let cape = CapeRenderer::new(width, height, half(head), half(avatar))?;
        let mut play = Play {
            take,
            start: Instant::now(),
            overlay,
            cape,
            switched: false,
            costs: Vec::with_capacity(160),
        };
        // The first frame at once: it stands in for the head.
        let first = play
            .cape
            .render(&take.moment(Duration::ZERO), Duration::ZERO);
        play.overlay
            .present(first)
            .inspect_err(|err| log::warn!("full mode: {err}"))
            .ok()?;
        Some(play)
    }

    fn draw_avatar(&mut self, scene: &Scene) {
        let (w, h) = self.window.avatar_size();
        if self.avatar.as_ref().map(HeadRenderer::size) != Some((w, h)) {
            self.avatar = HeadRenderer::new(w, h);
        }
        if let Some(renderer) = self.avatar.as_mut()
            && let Err(err) = self.window.set_avatar(renderer.render(scene).rgba)
        {
            log::warn!("full mode: {err}");
        }
    }

    fn reschedule(&self) {
        let period = if self.play.is_some() {
            FRAME_60_FPS
        } else if self.lively || self.window.is_front() {
            FRAME_30_FPS
        } else {
            BEHIND
        };
        self.ticker.set(Who::Full, Some(period));
    }

    /// Something the user said or typed, or a note (without the `›`).
    pub fn begin(&mut self, text: &str) {
        self.end_reply();
        if std::mem::take(&mut self.empty) {
            self.window.clear();
        }
        let style = if text.starts_with('›') {
            TextStyle::User
        } else {
            TextStyle::Label
        };
        self.window.append(&format!("{}\n", text.trim_end()), style);
    }

    /// The next piece of a reply.
    pub fn stream(&mut self, text: &str) {
        self.empty = false;
        let spans = self.reply.get_or_insert_default().push(text);
        self.write(spans);
    }

    /// Ends the reply, if one is being written.
    pub fn end_reply(&mut self) {
        let Some(mut reply) = self.reply.take() else {
            return;
        };
        let spans = reply.finish();
        self.write(spans);
        if let Some(code) = reply.last_code() {
            self.last_code = Some(code.to_owned());
        }
        self.window.append("\n\n", TextStyle::Body);
    }

    /// The latest code block.
    pub fn code(&self) -> Option<&str> {
        self.reply
            .as_ref()
            .and_then(Markdown::last_code)
            .or(self.last_code.as_deref())
    }

    /// Empties the conversation and says how things work.
    pub fn start_over(&mut self) {
        self.reply = None;
        self.last_code = None;
        self.window.clear();
        self.window.append(
            "Type below, or press Talk (Ctrl+M). Enter sends, Shift+Enter starts a new line.\n\
             Ctrl+Shift+C copies the last code, Ctrl+N starts a new chat, Esc leaves.\n",
            TextStyle::Label,
        );
        self.empty = true;
    }

    fn write(&self, spans: Vec<Span>) {
        for span in spans {
            self.window.append(&span.text, text_style(span.style));
        }
    }
}

impl Drop for FullMode {
    fn drop(&mut self) {
        self.ticker.set(Who::Full, None);
    }
}

fn text_style(style: Style) -> TextStyle {
    match style {
        Style::Body => TextStyle::Body,
        Style::Bold => TextStyle::Bold,
        Style::Heading => TextStyle::Heading,
        Style::InlineCode => TextStyle::InlineCode,
        Style::Code => TextStyle::Code,
        Style::Label => TextStyle::Label,
    }
}

fn half(r: RectPx) -> RectPx {
    RectPx {
        left: r.left / 2,
        top: r.top / 2,
        right: r.right / 2,
        bottom: r.bottom / 2,
    }
}

/// Logs what an animation's frames cost.
fn report(take: Take, mut costs: Vec<Duration>) {
    costs.sort_unstable();
    let p95 = costs.get(costs.len() * 95 / 100).copied();
    log::info!(
        "full mode: {take:?}, {} frames, p95 {:.1} ms, worst {:.1} ms",
        costs.len(),
        p95.unwrap_or_default().as_secs_f64() * 1000.0,
        costs.last().copied().unwrap_or_default().as_secs_f64() * 1000.0,
    );
}
