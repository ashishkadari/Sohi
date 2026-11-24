//! The head: an original "hooded ghost" netrunner bust inside a HUD frame
//! (spec §1.1, §4.1, §4.5).
//!
//! Everything is laid out on the 112 × 84 `HEAD_LAYOUT` grid and scaled
//! to the window's pixel size; "DIPs" below mean units of that grid. The
//! hood, face and shadow never change within a tone, so they are drawn once into a cached layer; each frame
//! copies that layer and adds the frame, eyes and collar line on top.

use std::time::Duration;

use sohi_core::geometry::{HEAD_LAYOUT, RectPx};
use sohi_core::visual::{ERROR_FLASH, VisualState};
use tiny_skia::{
    Color, FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform,
};

/// Spec §4.5 colour tokens (straight RGB).
pub(crate) mod tokens {
    pub const RED_ACTIVE: [u8; 3] = [0xB0, 0x12, 0x2A];
    pub const RED_IDLE: [u8; 3] = [0x6D, 0x0B, 0x1A];
    pub const GLOW: [u8; 3] = [0xFF, 0x4D, 0x5E];
    pub const OFFLINE: [u8; 3] = [0x6B, 0x55, 0x57];
    pub const AMBER: [u8; 3] = [0xFF, 0xB0, 0x20];
    pub const HOOD: [u8; 3] = [0x2A, 0x0B, 0x11];
    pub const HOOD_OFFLINE: [u8; 3] = [0x2A, 0x25, 0x26];
    pub const FACE: [u8; 3] = [0x0D, 0x04, 0x05];
    pub const GLOW_OFFLINE: [u8; 3] = [0x9A, 0x8F, 0x91];
    pub const BADGE: [u8; 3] = [0x8A, 0x84, 0x86];
}

/// Frame (corner brackets and hairline), in DIPs.
const FRAME: (f32, f32, f32, f32) = (12.0, 10.0, 88.0, 64.0);
/// Corner bracket arm length.
const BRACKET: f32 = 8.0;
/// Eye centres and base radius.
pub(crate) const EYES: [(f32, f32); 2] = [(52.0, 38.5), (60.0, 38.5)];
pub(crate) const EYE_RADIUS: f32 = 1.9;
/// Collar line centre, base width and height.
const COLLAR: (f32, f32) = (56.0, 64.7);
const COLLAR_WIDTH: f32 = 10.0;
const COLLAR_HEIGHT: f32 = 1.4;
/// Bounds (left, top, right, bottom) of everything the eyes and collar can
/// touch at any level, with a margin for anti-aliasing.
const FACE_REGION: (f32, f32, f32, f32) = (43.0, 31.0, 69.0, 68.0);

/// Everything that changes from frame to frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    /// What the head shows.
    pub state: VisualState,
    /// Time since the state began (drives the animations).
    pub t: Duration,
    /// 0–1: mic level while listening, voice envelope while speaking.
    pub level: f32,
    /// Time since an error, while its amber flash is running.
    pub error_age: Option<Duration>,
}

impl Scene {
    /// A scene with no level and no error.
    #[must_use]
    pub fn new(state: VisualState, t: Duration) -> Self {
        Self {
            state,
            t,
            level: 0.0,
            error_age: None,
        }
    }
}

/// The hood colour and opacity, which change only between states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    /// `RED_IDLE` at 90 %.
    Idle,
    /// `RED_ACTIVE`.
    Active,
    /// `RED_ACTIVE` at 85 %.
    Dim,
    /// `OFFLINE` grey.
    Offline,
}

impl Tone {
    fn of(state: VisualState) -> Self {
        match state {
            VisualState::Idle => Self::Idle,
            VisualState::Listening | VisualState::Speaking | VisualState::Confirming => {
                Self::Active
            }
            VisualState::Transcribing | VisualState::Thinking => Self::Dim,
            VisualState::Offline | VisualState::ForcedOffline => Self::Offline,
        }
    }

    fn rim(self) -> ([u8; 3], f32) {
        match self {
            Self::Idle => (tokens::RED_IDLE, 0.9),
            Self::Active => (tokens::RED_ACTIVE, 1.0),
            Self::Dim => (tokens::RED_ACTIVE, 0.85),
            Self::Offline => (tokens::OFFLINE, 1.0),
        }
    }
}

/// One rendered frame.
#[derive(Debug, Clone, Copy)]
pub struct Rendered<'a> {
    /// RGBA, premultiplied, `width * height * 4` bytes.
    pub rgba: &'a [u8],
    /// The only pixels that differ from the previous frame, or `None` if
    /// any may.
    pub dirty: Option<RectPx>,
}

/// Renders the head at one pixel size. Recreate it when the DPI changes.
pub struct HeadRenderer {
    scale: f32,
    pixmap: Pixmap,
    /// Cached hood, face and shadow for `body_tone`.
    body: Pixmap,
    body_tone: Option<Tone>,
    hood: Path,
    /// Everything outside the face region in the previous frame.
    prev_frame: Option<FrameKey>,
}

impl HeadRenderer {
    /// A renderer for a `width` × `height` pixel head window, or `None` if
    /// the size is zero.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Option<Self> {
        #[allow(clippy::cast_precision_loss)] // widths are far below 2^24
        let scale = width as f32 / HEAD_LAYOUT.0 as f32;
        Some(Self {
            scale,
            pixmap: Pixmap::new(width, height)?,
            body: Pixmap::new(width, height)?,
            body_tone: None,
            hood: hood_path(true)?,
            prev_frame: None,
        })
    }

    /// Pixel size this renderer draws.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.pixmap.width(), self.pixmap.height())
    }

    /// Draws `scene`. When only the eyes and collar changed since the last
    /// frame, [`Rendered::dirty`] names the face region so the caller can
    /// update just that part of the window.
    pub fn render(&mut self, scene: &Scene) -> Rendered<'_> {
        let tone = Tone::of(scene.state);
        if self.body_tone != Some(tone) {
            self.draw_body(tone);
            self.body_tone = Some(tone);
        }
        self.pixmap.data_mut().copy_from_slice(self.body.data());
        let look = Look::of(scene);
        let key = FrameKey::of(&look, tone);
        let dirty = (self.prev_frame == Some(key)).then(|| self.face_region_px());
        self.prev_frame = Some(key);
        let ts = Transform::from_scale(self.scale, self.scale);
        let pm = &mut self.pixmap;
        draw_frame(pm, ts, &look);
        draw_eyes(pm, ts, &look);
        draw_collar(pm, ts, &look);
        if let Some(y) = look.scanline {
            fill_rect(pm, ts, (FRAME.0, y, FRAME.2, 2.0), tokens::GLOW, 0.7);
        }
        if let Some(outlined) = look.badge {
            draw_badge(pm, ts, outlined);
        }
        Rendered {
            rgba: self.pixmap.data(),
            dirty,
        }
    }

    /// [`FACE_REGION`] in pixels, rounded outwards.
    fn face_region_px(&self) -> RectPx {
        let (left, top, right, bottom) = FACE_REGION;
        let px = |v: f32, up: bool| {
            let v = v * self.scale;
            let v = if up { v.ceil() } else { v.floor() };
            // Small positive values: the window is a few hundred pixels.
            #[allow(clippy::cast_possible_truncation)]
            let v = v as i32;
            v
        };
        RectPx {
            left: px(left, false),
            top: px(top, false),
            right: px(right, true),
            bottom: px(bottom, true),
        }
    }

    fn draw_body(&mut self, tone: Tone) {
        let ts = Transform::from_scale(self.scale, self.scale);
        let pm = &mut self.body;
        pm.fill(Color::TRANSPARENT);
        // Soft drop shadow: widening translucent strokes under a faint fill
        // (tiny-skia has no blur; this is drawn once per tone and size).
        let shadow = ts.pre_translate(0.0, 1.5);
        for (width, alpha) in [(6.0, 0.05), (4.0, 0.07), (2.0, 0.10)] {
            stroke(pm, &self.hood, shadow, [0, 0, 0], alpha, width, None);
        }
        fill(pm, &self.hood, shadow, [0, 0, 0], 0.12);

        let (rim, alpha) = tone.rim();
        let body = if tone == Tone::Offline {
            tokens::HOOD_OFFLINE
        } else {
            tokens::HOOD
        };
        // The body stays opaque so the desktop never shows through the
        // hood; the tone's opacity applies to the rim.
        fill(pm, &self.hood, ts, body, 1.0);
        if let Some(face) = rect(46.5, 27.5, 19.0, 25.0).and_then(PathBuilder::from_oval) {
            fill(pm, &face, ts, tokens::FACE, 1.0);
        }
        stroke(pm, &self.hood, ts, rim, alpha, 0.9, None);
        // Chest seam under the collar.
        let mut seam = PathBuilder::new();
        seam.move_to(56.0, 58.0);
        seam.line_to(56.0, 62.0);
        if let Some(seam) = seam.finish() {
            stroke(pm, &seam, ts, rim, 0.5 * alpha, 0.6, None);
        }
    }
}

/// The per-frame look derived from a scene.
struct Look {
    /// Frame colour.
    frame: [u8; 3],
    /// Bracket opacity, clockwise from top-left.
    brackets: [f32; 4],
    /// Hairline opacity and whether it is dashed.
    hairline: (f32, bool),
    /// Eye colour, opacity (brightness) and radius.
    eyes: ([u8; 3], f32, f32),
    /// Collar colour, opacity and width.
    collar: ([u8; 3], f32, f32),
    /// Scanline top edge, while transcribing.
    scanline: Option<f32>,
    /// Offline badge: `Some(false)` filled, `Some(true)` outlined.
    badge: Option<bool>,
}

impl Look {
    #[allow(clippy::too_many_lines)] // one arm per state reads best as a table
    fn of(scene: &Scene) -> Self {
        let t = scene.t.as_secs_f32();
        let level = scene.level.clamp(0.0, 1.0);
        let mut look = Self {
            frame: tokens::RED_ACTIVE,
            brackets: [1.0; 4],
            hairline: (0.3, false),
            eyes: (tokens::GLOW, 1.0, EYE_RADIUS),
            collar: (tokens::GLOW, 0.35, COLLAR_WIDTH),
            scanline: None,
            badge: None,
        };
        match scene.state {
            VisualState::Idle => {
                let breath = 1.0 + 0.08 * (t * std::f32::consts::TAU / 4.0).sin();
                look.brackets = [0.35; 4];
                look.hairline.0 = 0.2;
                look.eyes.1 = 0.6 * breath;
                look.collar.1 = 0.3 * breath;
            }
            VisualState::Listening => {
                look.eyes.2 = 1.5 + 1.1 * level;
            }
            VisualState::Transcribing => {
                look.brackets = [0.35; 4];
                look.eyes.1 = 0.4;
                look.scanline = Some(FRAME.1 + (FRAME.3 - 2.0) * ((t % 0.9) / 0.9));
            }
            VisualState::Thinking => {
                // One bracket lit at a time, clockwise, 600 ms per lap.
                let lit = phase_index(t, 0.6, 4);
                look.brackets = [0.2; 4];
                look.brackets[lit] = 1.0;
                look.eyes.1 = 0.4;
            }
            VisualState::Speaking => {
                look.collar = (
                    tokens::GLOW,
                    0.35 + 0.65 * level,
                    COLLAR_WIDTH + 14.0 * level,
                );
            }
            VisualState::Confirming => {
                look.frame = tokens::AMBER;
                // 2 Hz blink: on for 250 ms, off for 250 ms.
                look.eyes.1 = if phase_index(t, 0.5, 2) == 0 {
                    1.0
                } else {
                    0.15
                };
            }
            VisualState::Offline | VisualState::ForcedOffline => {
                look.frame = tokens::OFFLINE;
                look.brackets = [0.0; 4];
                look.hairline = (0.6, true);
                look.eyes = (tokens::GLOW_OFFLINE, 0.3, EYE_RADIUS);
                look.collar = (tokens::GLOW_OFFLINE, 0.2, COLLAR_WIDTH);
                look.badge = Some(scene.state == VisualState::ForcedOffline);
            }
        }
        if let Some(age) = scene.error_age
            && error_flash_on(age)
        {
            look.frame = tokens::AMBER;
            look.brackets = [1.0; 4];
        }
        look
    }
}

/// Everything a frame draws outside [`FACE_REGION`]; when it matches the
/// previous frame, only the face region can differ. Floats are compared
/// bit for bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameKey {
    tone: Tone,
    frame: [u8; 3],
    brackets: [u32; 4],
    hairline: (u32, bool),
    scanline: Option<u32>,
    badge: Option<bool>,
}

impl FrameKey {
    fn of(look: &Look, tone: Tone) -> Self {
        Self {
            tone,
            frame: look.frame,
            brackets: look.brackets.map(f32::to_bits),
            hairline: (look.hairline.0.to_bits(), look.hairline.1),
            scanline: look.scanline.map(f32::to_bits),
            badge: look.badge,
        }
    }
}

/// Whether the error overlay shows amber at `age`: on 0–120 ms and
/// 240–360 ms.
fn error_flash_on(age: Duration) -> bool {
    let ms = age.as_millis();
    age < ERROR_FLASH && !(120..240).contains(&ms)
}

/// Which of `steps` equal slices of a `period`-second cycle `t` falls in.
fn phase_index(t: f32, period: f32, steps: usize) -> usize {
    #[allow(clippy::cast_precision_loss)] // steps is tiny
    let slice = (t.rem_euclid(period) / period * steps as f32).floor();
    // In range: rem_euclid keeps the ratio in [0, 1).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let i = slice as usize;
    i.min(steps - 1)
}

fn draw_frame(pm: &mut Pixmap, ts: Transform, look: &Look) {
    let (x, y, w, h) = FRAME;
    let corners = [
        (x, y, 1.0, 1.0),
        (x + w, y, -1.0, 1.0),
        (x + w, y + h, -1.0, -1.0),
        (x, y + h, 1.0, -1.0),
    ];
    for ((cx, cy, dx, dy), alpha) in corners.into_iter().zip(look.brackets) {
        if alpha <= 0.0 {
            continue;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(cx, cy + BRACKET * dy);
        pb.line_to(cx, cy);
        pb.line_to(cx + BRACKET * dx, cy);
        if let Some(path) = pb.finish() {
            stroke(pm, &path, ts, look.frame, alpha, 1.0, None);
        }
    }
    let (alpha, dashed) = look.hairline;
    let dash = dashed
        .then(|| StrokeDash::new(vec![4.0, 3.0], 0.0))
        .flatten();
    if let Some(r) = rect(x, y, w, h) {
        stroke(
            pm,
            &PathBuilder::from_rect(r),
            ts,
            look.frame,
            alpha,
            1.0,
            dash,
        );
    }
}

fn draw_eyes(pm: &mut Pixmap, ts: Transform, look: &Look) {
    let (color, alpha, radius) = look.eyes;
    for (cx, cy) in EYES {
        if let Some(halo) = PathBuilder::from_circle(cx, cy, radius * 2.1) {
            fill(pm, &halo, ts, color, 0.22 * alpha);
        }
        if let Some(eye) = PathBuilder::from_circle(cx, cy, radius) {
            fill(pm, &eye, ts, color, alpha);
        }
    }
}

fn draw_collar(pm: &mut Pixmap, ts: Transform, look: &Look) {
    let (color, alpha, width) = look.collar;
    let (cx, cy) = COLLAR;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - width / 2.0, cy);
    pb.line_to(cx + width / 2.0, cy);
    if let Some(path) = pb.finish() {
        let s = Stroke {
            width: COLLAR_HEIGHT,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(color, alpha), &s, ts, None);
    }
}

fn draw_badge(pm: &mut Pixmap, ts: Transform, outlined: bool) {
    if outlined {
        if let Some(r) = rect(90.5, 64.5, 5.0, 5.0) {
            stroke(
                pm,
                &PathBuilder::from_rect(r),
                ts,
                tokens::BADGE,
                1.0,
                1.0,
                None,
            );
        }
    } else {
        fill_rect(pm, ts, (90.0, 64.0, 6.0, 6.0), tokens::BADGE, 1.0);
    }
}

/// The hood silhouette: shoulders at the frame's bottom edge rising to a
/// peaked hood; `closed` along the bottom or not.
pub(crate) fn hood_path(closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(30.0, 74.0);
    pb.quad_to(36.0, 58.0, 40.0, 46.0);
    pb.quad_to(41.0, 26.0, 56.0, 18.0);
    pb.quad_to(71.0, 26.0, 72.0, 46.0);
    pb.quad_to(76.0, 58.0, 82.0, 74.0);
    if closed {
        pb.close();
    }
    pb.finish()
}

/// A DIP rectangle (`None` only for a zero or negative size).
pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32) -> Option<Rect> {
    Rect::from_xywh(x, y, w, h)
}

fn paint(color: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round();
    // In range: clamped to [0, 255] above.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    p.set_color_rgba8(color[0], color[1], color[2], a as u8);
    p.anti_alias = true;
    p
}

pub(crate) fn fill(pm: &mut Pixmap, path: &Path, ts: Transform, color: [u8; 3], alpha: f32) {
    pm.fill_path(path, &paint(color, alpha), FillRule::Winding, ts, None);
}

fn fill_rect(pm: &mut Pixmap, ts: Transform, r: (f32, f32, f32, f32), color: [u8; 3], alpha: f32) {
    if let Some(r) = rect(r.0, r.1, r.2, r.3) {
        fill(pm, &PathBuilder::from_rect(r), ts, color, alpha);
    }
}

pub(crate) fn stroke(
    pm: &mut Pixmap,
    path: &Path,
    ts: Transform,
    color: [u8; 3],
    alpha: f32,
    width: f32,
    dash: Option<StrokeDash>,
) {
    let s = Stroke {
        width,
        dash,
        ..Stroke::default()
    };
    pm.stroke_path(path, &paint(color, alpha), &s, ts, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    /// Straight-alpha pixel at DIP point (`x`, `y`).
    fn px(r: &HeadRenderer, frame: &[u8], x: f32, y: f32) -> [u8; 4] {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (ix, iy) = ((x * r.scale) as usize, (y * r.scale) as usize);
        let i = (iy * r.pixmap.width() as usize + ix) * 4;
        [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
    }

    fn render(
        state: VisualState,
        t: Duration,
        level: f32,
        w: u32,
        h: u32,
    ) -> (HeadRenderer, Vec<u8>) {
        let mut r = HeadRenderer::new(w, h).unwrap();
        let scene = Scene {
            level,
            ..Scene::new(state, t)
        };
        let frame = r.render(&scene).rgba.to_vec();
        (r, frame)
    }

    /// Fails if any pixel outside `dirty` differs between the frames.
    fn assert_changes_inside(before: &[u8], after: &[u8], width: usize, dirty: RectPx) {
        for (i, (old, new)) in before.chunks(4).zip(after.chunks(4)).enumerate() {
            let col = i32::try_from(i % width).unwrap();
            let row = i32::try_from(i / width).unwrap();
            let inside =
                col >= dirty.left && col < dirty.right && row >= dirty.top && row < dirty.bottom;
            assert!(
                inside || old == new,
                "({col}, {row}) changed outside {dirty:?}"
            );
        }
    }

    #[test]
    fn idle_frames_change_only_inside_the_face_region() {
        let mut renderer = HeadRenderer::new(168, 126).unwrap();
        let first = renderer.render(&Scene::new(VisualState::Idle, Duration::ZERO));
        assert!(first.dirty.is_none(), "first frame is full");
        let before = first.rgba.to_vec();
        let second = renderer.render(&Scene::new(VisualState::Idle, MS(1000)));
        let dirty = second.dirty.expect("only the face changed");
        assert_ne!(before, second.rgba, "breathing changed something");
        assert_changes_inside(&before, second.rgba, 168, dirty);
    }

    #[test]
    fn listening_eyes_and_speaking_collar_stay_inside_the_face_region() {
        for state in [VisualState::Listening, VisualState::Speaking] {
            let mut renderer = HeadRenderer::new(224, 168).unwrap();
            let quiet = Scene::new(state, Duration::ZERO);
            let before = renderer.render(&quiet).rgba.to_vec();
            let loud = renderer.render(&Scene {
                level: 1.0,
                ..quiet
            });
            let dirty = loud.dirty.expect("only the face changed");
            assert_changes_inside(&before, loud.rgba, 224, dirty);
        }
    }

    #[test]
    fn state_change_and_bracket_chase_redraw_everything() {
        let mut r = HeadRenderer::new(168, 126).unwrap();
        r.render(&Scene::new(VisualState::Idle, Duration::ZERO));
        assert!(
            r.render(&Scene::new(VisualState::Thinking, Duration::ZERO))
                .dirty
                .is_none()
        );
        // The chase moves to the next bracket after 150 ms.
        assert!(
            r.render(&Scene::new(VisualState::Thinking, MS(200)))
                .dirty
                .is_none()
        );
    }

    #[test]
    #[allow(clippy::cast_precision_loss)] // small integers
    fn frame_matches_the_pill_anchor() {
        let (l, t, w, h) = sohi_core::pill::HEAD_FRAME_LAYOUT;
        assert_eq!(FRAME, (l as f32, t as f32, w as f32, h as f32));
    }

    #[test]
    fn frame_has_window_size_at_each_scale() {
        for (w, h) in [(112, 84), (168, 126), (224, 168)] {
            let (r, frame) = render(VisualState::Idle, Duration::ZERO, 0.0, w, h);
            assert_eq!(r.size(), (w, h));
            assert_eq!(frame.len(), (w * h * 4) as usize);
        }
    }

    #[test]
    fn corners_and_gap_beside_hood_are_click_through() {
        let (r, frame) = render(VisualState::Listening, Duration::ZERO, 0.5, 168, 126);
        assert_eq!(px(&r, &frame, 2.0, 2.0)[3], 0, "window corner");
        assert_eq!(px(&r, &frame, 20.0, 30.0)[3], 0, "between frame and hood");
    }

    #[test]
    fn hood_and_eyes_are_opaque() {
        let (r, frame) = render(VisualState::Speaking, Duration::ZERO, 0.0, 168, 126);
        assert!(px(&r, &frame, 56.0, 68.0)[3] > 200, "hood body");
        let eye = px(&r, &frame, 52.0, 38.5);
        assert!(eye[3] > 200 && eye[0] > 200, "eye glows red: {eye:?}");
    }

    #[test]
    fn listening_eyes_grow_with_level() {
        // Just outside the quiet radius, inside the loud one.
        let (r, quiet) = render(VisualState::Listening, Duration::ZERO, 0.0, 224, 168);
        let (_, loud) = render(VisualState::Listening, Duration::ZERO, 1.0, 224, 168);
        let probe = (52.0 + 2.2, 38.5);
        assert!(px(&r, &loud, probe.0, probe.1)[0] > px(&r, &quiet, probe.0, probe.1)[0]);
    }

    #[test]
    fn idle_breathes() {
        let (r, a) = render(VisualState::Idle, Duration::ZERO, 0.0, 168, 126);
        let (_, b) = render(VisualState::Idle, MS(1000), 0.0, 168, 126);
        assert_ne!(px(&r, &a, 52.0, 38.5), px(&r, &b, 52.0, 38.5));
    }

    #[test]
    fn thinking_lights_one_bracket_at_a_time() {
        let (r, f) = render(VisualState::Thinking, MS(10), 0.0, 168, 126);
        let tl = px(&r, &f, FRAME.0 + 4.0, FRAME.1)[3];
        let tr = px(&r, &f, FRAME.0 + FRAME.2 - 4.0, FRAME.1)[3];
        assert!(tl > tr, "top-left lit first: {tl} vs {tr}");
    }

    #[test]
    fn confirming_frame_is_amber() {
        let (r, f) = render(VisualState::Confirming, Duration::ZERO, 0.0, 168, 126);
        let c = px(&r, &f, FRAME.0 + 4.0, FRAME.1);
        assert!(c[1] > c[2], "amber has more green than blue: {c:?}");
    }

    #[test]
    fn offline_shows_badge_and_forced_offline_hollows_it() {
        let (r, off) = render(VisualState::Offline, Duration::ZERO, 0.0, 224, 168);
        let (_, forced) = render(VisualState::ForcedOffline, Duration::ZERO, 0.0, 224, 168);
        assert!(px(&r, &off, 93.0, 67.0)[3] > 200, "filled square");
        assert_eq!(px(&r, &forced, 93.0, 67.0)[3], 0, "outline only");
    }

    #[test]
    fn error_flashes_twice() {
        assert!(error_flash_on(MS(0)));
        assert!(!error_flash_on(MS(180)));
        assert!(error_flash_on(MS(300)));
        assert!(!error_flash_on(MS(400)));
    }

    #[test]
    fn phase_index_stays_in_range() {
        assert_eq!(phase_index(0.0, 0.6, 4), 0);
        assert_eq!(phase_index(0.16, 0.6, 4), 1);
        assert_eq!(phase_index(0.599, 0.6, 4), 3);
        assert_eq!(phase_index(-0.1, 0.6, 4), 3);
    }
}
