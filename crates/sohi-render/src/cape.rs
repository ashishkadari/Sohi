//! The cape that takes the screen over and goes back into the corner
//! (spec §4.9).
//!
//! Drawn at a fraction of the screen's resolution; the overlay window
//! enlarges it. All positions here are in those smaller pixels.

#![allow(clippy::many_single_char_names)] // coordinates

use std::f32::consts::PI;
use std::time::Duration;

use sohi_core::full::Moment;
use sohi_core::geometry::{HEAD_LAYOUT, RectPx};
use tiny_skia::{Color, Path, PathBuilder, Pixmap, Transform};

use crate::head::{EYE_RADIUS, EYES, fill, hood_path, rect, stroke, tokens};

/// Points along the hem.
const HEM: usize = 72;
/// The same, all the way around the head.
const AROUND: usize = 218;
/// How much later the far end of the hem moves, as a share of the sweep.
const LAG: f32 = 0.8;
/// The hem's directions as seen from the head: from a little above "left"
/// to a little right of "down". 0 is right, π/2 is down.
const FIRST: f32 = PI * 1.08;
const LAST: f32 = PI * 0.42;
/// The cloth, back to front: colour and how far it reaches.
const LAYERS: [([u8; 3], f32); 3] = [
    ([0x5A, 0x0B, 0x18], 1.0),
    (tokens::HOOD, 0.94),
    ([0x12, 0x05, 0x08], 0.88),
];
/// Reach at the end, in distances to the farthest corner: enough that the
/// deepest wave of the front layer is off the screen.
const REACH: f32 = 1.5;
/// The sweep at which the front layer hides everything, at every size.
const COVERED: f32 = 0.98;
/// How many folds the cloth has.
const FOLDS: usize = 9;
/// The hood's middle on the layout grid, and its height.
const FIGURE_CENTRE: (f32, f32) = (56.0, 45.0);
const FIGURE_HEIGHT: f32 = 56.0;

/// Draws the cape animations for one screen size.
pub struct CapeRenderer {
    pixmap: Pixmap,
    figure: Figure,
    cloak: Path,
    head: RectPx,
    avatar: RectPx,
}

impl CapeRenderer {
    /// A renderer for `width` × `height` pixels, with the head in the
    /// corner at `head` and the one in the window's header at `avatar`.
    /// `None` if the size is zero.
    #[must_use]
    pub fn new(width: u32, height: u32, head: RectPx, avatar: RectPx) -> Option<Self> {
        Some(Self {
            pixmap: Pixmap::new(width, height)?,
            figure: Figure {
                hood: hood_path(true)?,
                rim: hood_path(false)?,
            },
            cloak: cloak_path()?,
            head,
            avatar,
        })
    }

    /// Pixel size this renderer draws.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.pixmap.width(), self.pixmap.height())
    }

    /// Draws `m`, `t` after the start. Returns premultiplied RGBA.
    pub fn render(&mut self, m: &Moment, t: Duration) -> &[u8] {
        let t = t.as_secs_f32();
        self.pixmap.fill(Color::TRANSPARENT);
        if m.sweep > 0.0 && m.cape > 0.0 {
            self.draw_cape(m, t);
        }
        if m.head > 0.0 {
            let ts = fit(self.head);
            self.figure.draw(&mut self.pixmap, ts, m.turn, 0.7, m.head);
        }
        if m.figure > 0.0 {
            self.draw_figure(m);
        }
        scanlines(&mut self.pixmap);
        self.pixmap.data()
    }

    /// Where the cape hangs from.
    fn shoulders(&self) -> (f32, f32) {
        let h = self.head;
        (px(h.left + h.right) / 2.0, px(h.top + h.bottom) / 2.0)
    }

    fn draw_cape(&mut self, m: &Moment, t: f32) {
        let (ax, ay) = self.shoulders();
        let (w, h) = (
            self.pixmap.width().cast_signed(),
            self.pixmap.height().cast_signed(),
        );
        let far = (px(w).max(ax)).hypot(px(h).max(ay));
        let hem = |reach: f32, shift: f32| -> Vec<(f32, f32)> {
            (0..AROUND)
                .map(|i| {
                    let along = px(i) / px(HEM);
                    let angle = FIRST + (LAST - FIRST) * along;
                    // Thrown to the left first; the rest falls after it.
                    // Behind the head, where little is seen, it catches up.
                    let late = if i <= HEM {
                        along
                    } else {
                        px(AROUND - i) / px(AROUND - HEM)
                    };
                    let part = (m.sweep * (1.0 + LAG) - LAG * late).clamp(0.0, 1.0);
                    let wave = 0.055 * (along * 17.0 - t * 9.0 + shift).sin()
                        + 0.04 * (along * 31.0 + t * 12.0 + shift * 2.0).sin()
                        + 0.025 * (along * 59.0 - t * 15.0).sin();
                    let r = far * REACH * reach * part * (1.0 + wave);
                    (ax + r * angle.cos(), ay + r * angle.sin())
                })
                .collect()
        };
        let id = Transform::identity();
        let front = LAYERS.len() - 1;
        for (i, (color, reach)) in LAYERS.into_iter().enumerate() {
            let edge = hem(reach, px(i) * 1.7);
            let out = |&(x, y): &(f32, f32)| x < 0.0 || y < 0.0 || x > px(w) || y > px(h);
            let all_out = edge.iter().take(HEM + 1).all(out) && m.sweep > 0.5;
            if i == front && all_out {
                let [r, g, b] = color;
                self.pixmap.fill(Color::from_rgba8(r, g, b, 255));
            } else if i < front && m.sweep >= COVERED {
                // The front layer is about to hide it.
                continue;
            } else if let Some(cloth) = polyline(&edge) {
                fill(&mut self.pixmap, &cloth, id, color, 1.0);
            }
            if i == front {
                self.draw_folds((ax, ay), &edge, t);
            }
            if !all_out && let Some(line) = edge.get(..=HEM).and_then(polyline) {
                let glow = if i == 0 { 0.9 } else { 0.25 };
                let pm = &mut self.pixmap;
                stroke(pm, &line, id, tokens::GLOW, glow * 0.16, 7.0, None);
                stroke(pm, &line, id, tokens::GLOW, glow, 1.2, None);
            }
        }
        fade(&mut self.pixmap, m.cape);
    }

    /// Lighter wedges from the shoulders to the hem, bending as they go.
    fn draw_folds(&mut self, from: (f32, f32), edge: &[(f32, f32)], t: f32) {
        let step = HEM / FOLDS;
        for fold in 0..FOLDS {
            let start = fold * step + 2;
            let end = start + step / 2;
            let (Some(&a), Some(&b)) = (edge.get(start), edge.get(end)) else {
                continue;
            };
            // Not fallen yet.
            if [a, b]
                .iter()
                .any(|p| (p.0 - from.0).hypot(p.1 - from.1) < 2.0)
            {
                continue;
            }
            let sway = 0.13 * (t * 4.0 + px(fold) * 1.3).sin();
            // Each side is an S: cloth that ripples, not a ray.
            let bend = |p: (f32, f32), at: f32, sway: f32| {
                let (dx, dy) = (p.0 - from.0, p.1 - from.1);
                (from.0 + dx * at - dy * sway, from.1 + dy * at + dx * sway)
            };
            let mut pb = PathBuilder::new();
            pb.move_to(from.0, from.1);
            let (c, d) = (bend(a, 0.2, sway), bend(a, 0.55, -sway));
            pb.cubic_to(c.0, c.1, d.0, d.1, a.0, a.1);
            pb.line_to(b.0, b.1);
            let (c, d) = (bend(b, 0.55, -sway * 0.7), bend(b, 0.2, sway * 0.7));
            pb.cubic_to(c.0, c.1, d.0, d.1, from.0, from.1);
            pb.close();
            if let Some(path) = pb.finish() {
                let id = Transform::identity();
                let light = if fold % 2 == 0 { 0.30 } else { 0.16 };
                fill(&mut self.pixmap, &path, id, tokens::RED_IDLE, light);
            }
        }
    }

    /// The figure: big in the middle, then on its way to the header.
    fn draw_figure(&mut self, m: &Moment) {
        let (w, h) = (
            px(self.pixmap.width().cast_signed()),
            px(self.pixmap.height().cast_signed()),
        );
        let big = 0.52 * h / FIGURE_HEIGHT;
        let rise = (1.0 - m.figure) * 0.04 * h * (1.0 - m.travel);
        let end = fit(self.avatar);
        let mix = |a: f32, b: f32| a + (b - a) * m.travel;
        let scale = mix(big, end.sx);
        let x = mix(w / 2.0, end.tx + FIGURE_CENTRE.0 * end.sx);
        let y = mix(h * 0.5 + rise, end.ty + FIGURE_CENTRE.1 * end.sy);
        let ts = Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            x - FIGURE_CENTRE.0 * scale,
            y - FIGURE_CENTRE.1 * scale,
        );
        // The cloak under the shoulders goes first: the header has only
        // the head.
        let cloak = m.figure * (1.0 - m.travel * 3.0).max(0.0);
        if cloak > 0.0 {
            fill(&mut self.pixmap, &self.cloak, ts, [0x1C, 0x07, 0x0C], cloak);
            stroke(
                &mut self.pixmap,
                &self.cloak,
                ts,
                tokens::RED_IDLE,
                0.8 * cloak,
                0.5,
                None,
            );
        }
        // It looks out from under the hood as its eyes open.
        let turn = (1.0 - m.eyes) * 0.8;
        self.figure
            .draw(&mut self.pixmap, ts, turn, m.eyes, m.figure);
    }
}

/// The transform from the head's layout grid into `r`.
fn fit(r: RectPx) -> Transform {
    let scale = px(r.right - r.left) / px(HEAD_LAYOUT.0.cast_signed());
    Transform::from_row(scale, 0.0, 0.0, scale, px(r.left), px(r.top))
}

/// The hooded head's outlines.
struct Figure {
    hood: Path,
    /// Without the line under the shoulders: the cloak goes on below.
    rim: Path,
}

impl Figure {
    /// Draws it turned `turn` (0 to 1) towards the screen.
    fn draw(&self, pm: &mut Pixmap, ts: Transform, turn: f32, eyes: f32, alpha: f32) {
        fill(pm, &self.hood, ts, tokens::HOOD, alpha);
        let shift = -5.0 * turn;
        if let Some(face) =
            rect(46.5 + shift, 27.5, 19.0 - 2.5 * turn, 25.0).and_then(PathBuilder::from_oval)
        {
            fill(pm, &face, ts, tokens::FACE, alpha);
        }
        stroke(pm, &self.rim, ts, tokens::RED_ACTIVE, alpha, 0.9, None);
        // The hood's far side folds over as it turns.
        let mut pb = PathBuilder::new();
        pb.move_to(56.0, 18.5);
        pb.quad_to(68.0 - 4.0 * turn, 30.0, 68.0 - 2.0 * turn, 58.0);
        if turn > 0.0
            && let Some(fold) = pb.finish()
        {
            stroke(
                pm,
                &fold,
                ts,
                tokens::RED_IDLE,
                0.8 * turn * alpha,
                0.8,
                None,
            );
        }
        if let Some(collar) = rect(50.0 + shift * 0.5, 64.0, 12.0, 1.4) {
            let collar = PathBuilder::from_rect(collar);
            fill(pm, &collar, ts, tokens::GLOW, 0.45 * eyes * alpha);
        }
        if eyes < 0.02 {
            return;
        }
        for (i, (x, y)) in EYES.into_iter().enumerate() {
            // The farther eye narrows.
            let x = x + shift * 1.25 + if i == 0 { 0.6 * turn } else { 0.0 };
            let size = EYE_RADIUS * if i == 0 { 1.0 - 0.3 * turn } else { 1.0 };
            for (radius, light) in [(size * 2.6, 0.16), (size * 1.8, 0.2), (size, 1.0)] {
                let Some(eye) = rect(
                    x - radius,
                    y - radius * eyes,
                    2.0 * radius,
                    2.0 * radius * eyes,
                )
                .and_then(PathBuilder::from_oval) else {
                    continue;
                };
                fill(pm, &eye, ts, tokens::GLOW, light * alpha);
            }
        }
    }
}

fn polyline(points: &[(f32, f32)]) -> Option<Path> {
    let first = points.first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in points {
        pb.line_to(p.0, p.1);
    }
    pb.finish()
}

/// The cloak below the hood's shoulders, down out of sight.
fn cloak_path() -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(30.2, 73.4);
    pb.quad_to(18.0, 96.0, 8.0, 150.0);
    pb.line_to(104.0, 150.0);
    pb.quad_to(94.0, 96.0, 81.8, 73.4);
    pb.close();
    pb.finish()
}

/// Multiplies everything by `alpha`.
fn fade(pm: &mut Pixmap, alpha: f32) {
    if alpha >= 1.0 {
        return;
    }
    // In range: alpha is clamped to [0, 1].
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let alpha = (alpha.clamp(0.0, 1.0) * 256.0) as u16;
    for c in pm.data_mut() {
        *c = u8::try_from((u16::from(*c) * alpha) >> 8).unwrap_or(255);
    }
}

/// Darkens every third row a little, as on the head's hologram.
fn scanlines(pm: &mut Pixmap) {
    let stride = pm.width() as usize * 4;
    for row in pm.data_mut().chunks_exact_mut(stride).step_by(3) {
        for [r, g, b, _] in row.as_chunks_mut::<4>().0 {
            for c in [r, g, b] {
                *c -= *c / 5;
            }
        }
    }
}

/// A count or coordinate, all far below 2^24.
#[allow(clippy::cast_precision_loss)]
fn px<T: TryInto<i32>>(v: T) -> f32 {
    v.try_into().unwrap_or(0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use sohi_core::full::Take;

    const HEAD: RectPx = RectPx {
        left: 850,
        top: 10,
        right: 950,
        bottom: 85,
    };
    const AVATAR: RectPx = RectPx {
        left: 100,
        top: 5,
        right: 226,
        bottom: 100,
    };

    fn renderer() -> CapeRenderer {
        CapeRenderer::new(960, 600, HEAD, AVATAR).unwrap()
    }

    fn alphas(data: &[u8]) -> impl Iterator<Item = u8> {
        data.as_chunks::<4>().0.iter().map(|p| p[3])
    }

    #[test]
    fn nothing_shows_through_once_covered() {
        for (w, h) in [(960, 600), (640, 400), (1280, 540)] {
            for take in [Take::Over, Take::Back] {
                for ms in [0, 4, 9] {
                    let at = take.covered_at() + Duration::from_millis(ms);
                    let mut r = CapeRenderer::new(w, h, HEAD, AVATAR).unwrap();
                    let m = take.moment(at);
                    let holes = alphas(r.render(&m, at)).filter(|&a| a < 255).count();
                    assert_eq!(holes, 0, "{take:?} {w}x{h} at {at:?}");
                }
            }
        }
    }

    #[test]
    fn it_starts_with_the_head_alone_and_ends_empty() {
        let mut r = renderer();
        let at = Duration::ZERO;
        let data = r.render(&Take::Over.moment(at), at).to_vec();
        for (i, a) in alphas(&data).enumerate() {
            let (x, y) = (i % 960, i / 960);
            if !(845..955).contains(&x) || y > 90 {
                assert_eq!(a, 0, "at {x},{y}");
            }
        }
        assert!(alphas(&data).any(|a| a == 255));
        let at = Take::Over.duration();
        assert!(alphas(r.render(&Take::Over.moment(at), at)).all(|a| a == 0));
    }

    #[test]
    fn the_sweep_grows_steadily() {
        let mut r = renderer();
        let mut last = 0;
        for ms in (300..=1050).step_by(75) {
            let at = Duration::from_millis(ms);
            let covered = alphas(r.render(&Take::Over.moment(at), at))
                .filter(|&a| a > 0)
                .count();
            // The waves may take back a little.
            assert!(covered + 6_000 >= last, "{covered} < {last} at {ms} ms");
            last = covered;
        }
    }

    #[test]
    fn colours_stay_premultiplied() {
        let mut r = renderer();
        let at = Duration::from_millis(1700);
        for [red, g, b, a] in r.render(&Take::Over.moment(at), at).as_chunks::<4>().0 {
            assert!(red <= a && g <= a && b <= a);
        }
    }
}
