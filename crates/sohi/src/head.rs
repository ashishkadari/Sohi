//! The head: its window, renderer and look, and the Visibility region of
//! the state machine (spec §4.3–§4.5, §5.1).
//!
//! [`Head::input`] feeds `sohi_core::visibility` and carries out what it
//! asks for: start a slide (reversing one already running from wherever
//! the head is), or hide the window once parked. The frame timer ticks only
//! while something moves, at the visual state's rate while shown, or once
//! at the auto-retract deadline for a static state.

use std::rc::Rc;
use std::time::{Duration, Instant};

use sohi_core::config::ReducedMotion;
use sohi_core::geometry::RectPx;
use sohi_core::meter::{Envelope, display_level};
use sohi_core::motion::{Pose, Slide};
use sohi_core::visibility::{Effect, Input, Phase, Visibility};
use sohi_core::visual::{ERROR_FLASH, VisualState};
use sohi_core::wake::Gate;
use sohi_render::{HeadRenderer, Scene};
use sohi_win::{EventQueue, FRAME_30_FPS, FRAME_60_FPS, Frame, HeadWindow, WinError};

use crate::dev::{self, TestCardPulse, percentile};
use crate::ticker::{Ticker, Who};

/// A timer tick this close to a deadline counts as reaching it.
const TICK_SLACK: Duration = Duration::from_millis(8);

/// A running slide.
#[derive(Debug, Clone, Copy)]
struct Motion {
    slide: Slide,
    start: Instant,
}

impl Motion {
    /// Where the head is at `now`; past the end, where the slide ends.
    fn pose_at(self, now: Instant, reduced: bool) -> Option<Pose> {
        self.slide
            .pose(now.saturating_duration_since(self.start), reduced)
    }

    fn end_pose(self) -> Pose {
        match self.slide {
            Slide::Reveal => Pose::SHOWN,
            Slide::Retract => Pose {
                offset: 1.0,
                opacity: 0.0,
            },
        }
    }
}

/// `--perf` bookkeeping for head frames.
#[derive(Default)]
struct PerfTrace {
    /// The first visible frame has been reported.
    first_frame_done: bool,
    /// Present cost of each frame since the last report.
    costs: Vec<Duration>,
}

/// The head widget and everything that decides what it shows.
pub struct Head {
    window: HeadWindow,
    /// Shared with the message loop; ticks only while something animates.
    timer: Rc<Ticker>,
    /// Period the frame timer is ticking at, if any.
    tick: Option<Duration>,
    /// Window rectangle in physical pixels.
    rect: RectPx,
    /// Draws the head; recreated when the window size changes.
    renderer: Option<HeadRenderer>,
    visibility: Visibility,
    /// The slide in progress, if any.
    motion: Option<Motion>,
    /// The user's reduced-motion preference.
    reduced_pref: ReducedMotion,
    /// Reduced motion, decided when the current reveal started.
    reduced: bool,
    /// What the head shows.
    visual: VisualState,
    /// When `visual` began (its animation clock).
    visual_since: Instant,
    /// When the last error happened (drives the amber flash).
    error_at: Option<Instant>,
    /// `--dev-states`: when the cycle through every state began.
    dev_cycle: Option<Instant>,
    /// Running `--dev-test-card` pulse, if any.
    test_card: Option<TestCardPulse>,
    /// Whether the static test card is on screen (redrawn on DPI change).
    showing_test_card: bool,
    /// Perf mode only.
    perf: Option<PerfTrace>,
    /// The latest mic level (display scale, 0–1) while listening.
    level_target: f32,
    /// The smoothed level the visor shows, and when it was last stepped.
    level: (Envelope, Option<Instant>),
}

impl Head {
    /// Creates the (hidden, parked) head window.
    ///
    /// # Errors
    /// Returns [`WinError`] if the window cannot be created.
    pub fn new(
        queue: &EventQueue,
        timer: Rc<Ticker>,
        reduced_pref: ReducedMotion,
        perf: bool,
    ) -> Result<Self, WinError> {
        Ok(Self {
            window: HeadWindow::create(queue.clone())?,
            timer,
            tick: None,
            rect: RectPx::default(),
            renderer: None,
            visibility: Visibility::new(),
            motion: None,
            reduced_pref,
            reduced: false,
            visual: VisualState::Idle,
            visual_since: Instant::now(),
            error_at: None,
            dev_cycle: None,
            level_target: 0.0,
            level: (Envelope::default(), None),
            test_card: None,
            showing_test_card: false,
            perf: perf.then(PerfTrace::default),
        })
    }

    /// Where the head is in its reveal/retract cycle.
    pub fn phase(&self) -> Phase {
        self.visibility.phase()
    }

    /// Whether a corner dwell may reveal the head now: only when parked,
    /// and only once `cooldown` has passed since the last retract (§4.7).
    pub fn corner_gate(&self, now: Instant, cooldown: Duration) -> Gate {
        if self.phase() != Phase::Parked {
            Gate::Deny
        } else if self.visibility.corner_wake_allowed(now, cooldown) {
            Gate::Allow
        } else {
            Gate::Wait
        }
    }

    /// Feeds the Visibility region and carries out its effect.
    pub fn input(&mut self, input: Input) {
        let now = Instant::now();
        if let Some(effect) = self.visibility.step(now, input) {
            self.apply(now, effect);
        }
        self.reschedule(now);
    }

    /// Moves or resizes the head. Returns `true` if anything changed.
    pub fn set_rect(&mut self, rect: RectPx) -> bool {
        if rect == self.rect {
            return false;
        }
        self.window.set_rect(rect);
        self.rect = rect;
        self.prepare_renderer();
        if self.phase() == Phase::Shown && self.motion.is_none() {
            // Static content: redraw once at the new size and position.
            self.present_pose(Pose::SHOWN);
        }
        if self.showing_test_card
            && self.test_card.is_none()
            && let Err(err) = self.draw_test_card(128)
        {
            log::warn!("test card redraw failed: {err}");
        }
        true
    }

    /// The level (RMS, 0–1) of the mic while listening, or of Sohi's
    /// speech while speaking; the visor follows it with a 30 ms attack and
    /// 150 ms release (spec §4.2).
    pub fn set_level(&mut self, rms: f32) {
        self.level_target = display_level(rms);
    }

    /// The visor level for a frame drawn at `now`.
    fn visor_level(&mut self, now: Instant) -> f32 {
        if !matches!(self.visual, VisualState::Listening | VisualState::Speaking) {
            return 0.0;
        }
        let (envelope, last) = &mut self.level;
        let dt = last.map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        *last = Some(now);
        envelope.step(self.level_target, dt)
    }

    /// What the head shows.
    pub fn visual(&self) -> VisualState {
        self.visual
    }

    /// Changes what the head shows. A fully shown head redraws at once.
    pub fn set_visual(&mut self, visual: VisualState) {
        if visual == self.visual {
            return;
        }
        log::info!("visual: {}", visual.name());
        self.visual = visual;
        self.visual_since = Instant::now();
        self.level_target = 0.0;
        self.level = (Envelope::default(), None);
        if self.phase() == Phase::Shown && self.motion.is_none() {
            self.present_pose(Pose::SHOWN);
            self.reschedule(Instant::now());
        }
    }

    /// Starts the two amber error flashes over the current state.
    pub fn flash_error(&mut self) {
        let now = Instant::now();
        self.error_at = Some(now);
        self.reschedule(now);
    }

    /// `--dev-states`: reveals the head and shows every visual state in
    /// turn, with a synthetic mic/voice level, then retracts.
    pub fn start_dev_cycle(&mut self) {
        self.dev_cycle = Some(Instant::now());
        self.set_visual(VisualState::ALL[0]);
        self.input(Input::Reveal {
            retract_after: Some(dev::cycle_length()),
        });
    }

    /// Shows the neutral test card and pulses it at 30 fps for a few
    /// seconds (`--dev-test-card`), then leaves it static.
    ///
    /// # Errors
    /// Returns [`WinError`] if the first frame cannot be presented.
    pub fn show_test_card(&mut self) -> Result<(), WinError> {
        self.draw_test_card(0)?;
        self.window.show();
        self.showing_test_card = true;
        self.test_card = Some(TestCardPulse::new(Instant::now()));
        self.set_tick(Some(FRAME_30_FPS));
        Ok(())
    }

    /// Handles a frame-timer tick.
    pub fn on_frame(&mut self) {
        if self.test_card.is_some() {
            self.step_test_card();
            return;
        }
        let now = Instant::now();
        if let Some(motion) = self.motion {
            match motion.pose_at(now, self.reduced) {
                Some(pose) => self.present_pose(pose),
                None => self.finish_slide(now, motion),
            }
        } else if self.phase() == Phase::Shown {
            if let Some(effect) = self.visibility.step(now + TICK_SLACK, Input::Tick) {
                self.apply(now, effect);
            } else {
                self.step_dev_cycle(now);
                if self.frame_period(now).is_some() {
                    self.present_pose(Pose::SHOWN);
                }
            }
        }
        self.reschedule(now);
    }

    fn finish_slide(&mut self, now: Instant, motion: Motion) {
        self.motion = None;
        if motion.slide == Slide::Reveal {
            self.present_pose(Pose::SHOWN);
        }
        if let Some(effect) = self.visibility.step(now, Input::SlideDone) {
            self.apply(now, effect);
        }
    }

    fn apply(&mut self, now: Instant, effect: Effect) {
        match effect {
            Effect::SlideIn if self.motion.is_none() => self.reveal_from_parked(now),
            Effect::SlideIn => self.start_motion(now, Slide::Reveal),
            Effect::SlideOut => self.start_motion(now, Slide::Retract),
            Effect::Hide => self.park(),
        }
    }

    /// Whether to leave movement out.
    pub fn reduced_motion(&self) -> bool {
        match self.reduced_pref {
            ReducedMotion::System => !sohi_win::animations_enabled(),
            ReducedMotion::On => true,
            ReducedMotion::Off => false,
        }
    }

    /// Parks without sliding: something else takes the head's place.
    pub fn vanish(&mut self) {
        if self.phase() != Phase::Parked {
            self.input(Input::Retract);
            self.skip_slide();
        }
    }

    /// Shows without sliding, for `hold`.
    pub fn appear(&mut self, hold: Duration) {
        self.input(Input::Reveal {
            retract_after: Some(hold),
        });
        self.skip_slide();
    }

    fn skip_slide(&mut self) {
        if let Some(motion) = self.motion {
            let now = Instant::now();
            self.finish_slide(now, motion);
            self.reschedule(now);
        }
    }

    fn reveal_from_parked(&mut self, now: Instant) {
        self.reduced = self.reduced_motion();
        log::info!("reveal (reduced motion: {})", self.reduced);
        self.motion = Some(Motion {
            slide: Slide::Reveal,
            start: now,
        });
        // Draw the clear first pose before showing, so no stale pixels
        // flash.
        let first = Slide::Reveal
            .pose(Duration::ZERO, self.reduced)
            .unwrap_or(Pose::SHOWN);
        self.present_pose(first);
        self.window.show();
    }

    /// Starts `slide` from wherever the head is now (shown, or partway
    /// through the opposite slide).
    fn start_motion(&mut self, now: Instant, slide: Slide) {
        let from = self.motion.map_or(Pose::SHOWN, |m| {
            m.pose_at(now, self.reduced).unwrap_or_else(|| m.end_pose())
        });
        let into = slide.resume_at(from, self.reduced);
        log::info!("{slide:?} from {:.0}% shown", from.opacity * 100.0);
        self.motion = Some(Motion {
            slide,
            start: now.checked_sub(into).unwrap_or(now),
        });
    }

    fn park(&mut self) {
        self.window.hide();
        self.motion = None;
        self.report_frame_costs();
        if self.dev_cycle.take().is_some() {
            self.set_visual(VisualState::Idle);
        }
        log::info!("parked");
    }

    /// Ticks at 60 fps while sliding; while shown, at the state's rate or,
    /// for a static state, once at the auto-retract deadline; never while
    /// parked.
    fn reschedule(&mut self, now: Instant) {
        if self.test_card.is_some() {
            return;
        }
        let period = if self.motion.is_some() {
            Some(FRAME_60_FPS)
        } else if self.phase() == Phase::Shown {
            self.frame_period(now).or_else(|| {
                self.visibility
                    .next_deadline()
                    .map(|at| at.saturating_duration_since(now).max(TICK_SLACK))
            })
        } else {
            None
        };
        self.set_tick(period);
    }

    /// The frame period for what is on screen now, or `None` if static.
    fn frame_period(&self, now: Instant) -> Option<Duration> {
        let flashing = self
            .error_at
            .is_some_and(|at| now.saturating_duration_since(at) < ERROR_FLASH);
        if flashing {
            return Some(FRAME_60_FPS);
        }
        if self.dev_cycle.is_some() && self.visual.fps().is_none() {
            // The dev cycle must keep ticking to leave a static state.
            return Some(FRAME_30_FPS);
        }
        match self.visual.fps()? {
            fps if fps > 30 => Some(FRAME_60_FPS),
            _ => Some(FRAME_30_FPS),
        }
    }

    /// Starts, retimes or stops the frame timer, only when the period
    /// actually changes (restarting every tick would drift).
    fn set_tick(&mut self, period: Option<Duration>) {
        if period == self.tick {
            return;
        }
        self.tick = period;
        self.timer.set(Who::Head, period);
    }

    /// `--dev-states`: moves to the state for this point in the cycle, and
    /// flashes an error once partway through Thinking.
    fn step_dev_cycle(&mut self, now: Instant) {
        let Some(start) = self.dev_cycle else {
            return;
        };
        let state = dev::cycle_state(now.saturating_duration_since(start));
        self.set_visual(state);
        let in_state = now.saturating_duration_since(self.visual_since);
        if state == VisualState::Thinking && in_state >= dev::ERROR_AT && self.error_at.is_none() {
            self.flash_error();
        }
    }

    /// What to draw now. Steps the visor: only one place may draw at a
    /// time (the head window, or full mode).
    pub fn scene(&mut self) -> Scene {
        let now = Instant::now();
        let visor = self.visor_level(now);
        let t = now.saturating_duration_since(self.visual_since);
        Scene {
            level: if self.dev_cycle.is_some() {
                dev::fake_level(t)
            } else {
                visor
            },
            error_age: self
                .error_at
                .map(|at| now.saturating_duration_since(at))
                .filter(|age| *age < ERROR_FLASH),
            ..Scene::new(self.visual, t)
        }
    }

    /// Presents the head pixels at `pose`.
    fn present_pose(&mut self, pose: Pose) {
        let started = Instant::now();
        let (w, h) = (self.rect.width(), self.rect.height());
        let scene = self.scene();
        self.prepare_renderer();
        let Some(renderer) = self.renderer.as_mut() else {
            log::warn!("cannot draw a {w}x{h} head");
            return;
        };
        let pixels = renderer.render(&scene);
        let frame = Frame {
            width: w,
            height: h,
            rgba: pixels.rgba,
            dirty: pixels.dirty,
        };
        let alpha = pose.alpha_u8();
        let result = self.window.present(
            frame,
            self.rect.left,
            self.rect.top,
            alpha,
            pose.shift_px(w),
        );
        if let Err(err) = result {
            log::warn!("head frame failed: {err}");
            return;
        }
        if let Some(trace) = self.perf.as_mut() {
            trace.costs.push(started.elapsed());
            if alpha > 0 && !trace.first_frame_done {
                trace.first_frame_done = true;
                crate::perf::on_first_frame();
            }
        }
    }

    /// Makes sure a renderer for the current window size exists and its
    /// cached hood is drawn, so that one-off work stays off the frame path.
    fn prepare_renderer(&mut self) {
        let size = (self.rect.width(), self.rect.height());
        if self.renderer.as_ref().map(HeadRenderer::size) == Some(size) {
            return;
        }
        self.renderer = HeadRenderer::new(size.0, size.1);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.render(&Scene::new(self.visual, Duration::ZERO));
        }
    }

    /// Perf mode: logs the present cost of the frames since the last report.
    fn report_frame_costs(&mut self) {
        let Some(costs) = self.perf.as_mut().map(|t| &mut t.costs) else {
            return;
        };
        if costs.is_empty() {
            return;
        }
        costs.sort_unstable();
        crate::perf::on_head_frames(costs.len(), percentile(costs, 95), costs[costs.len() - 1]);
        costs.clear();
    }

    fn step_test_card(&mut self) {
        let now = Instant::now();
        let Some(level) = self.test_card.as_ref().and_then(|p| p.level(now)) else {
            // Static from here on: the last frame stays, with zero redraws.
            self.set_tick(None);
            if let Some(pulse) = self.test_card.take() {
                log::info!("test card pulse done: {}", pulse.report());
            }
            return;
        };
        if let Err(err) = self.draw_test_card(level) {
            log::warn!("test card frame failed: {err}");
        }
        if let Some(pulse) = self.test_card.as_mut() {
            pulse.record(now.elapsed());
        }
    }

    fn draw_test_card(&mut self, level: u8) -> Result<(), WinError> {
        let (w, h) = (self.rect.width(), self.rect.height());
        let pixels = sohi_render::test_card(w, h, level);
        let frame = Frame {
            width: w,
            height: h,
            rgba: &pixels,
            dirty: None,
        };
        self.window
            .present(frame, self.rect.left, self.rect.top, 255, 0)
    }
}
