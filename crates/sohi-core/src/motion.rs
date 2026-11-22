//! Reveal and retract motion (spec §4.3). Pure: callers pass the elapsed
//! time and get back where the content should be drawn.
//!
//! The window itself never moves. The content is shifted right inside the
//! bitmap (and clipped by the window edge), so it looks like it slides in
//! from off-screen without ever touching a monitor to the right.

use std::time::Duration;

/// Reveal length (cubic ease-out).
pub const REVEAL: Duration = Duration::from_millis(260);
/// Retract length (cubic ease-in).
pub const RETRACT: Duration = Duration::from_millis(200);
/// Fade length when reduced motion is on (no translation).
pub const REDUCED_FADE: Duration = Duration::from_millis(60);
/// How long the launch peek stays fully shown before retracting.
pub const LAUNCH_PEEK_HOLD: Duration = Duration::from_millis(1200);

/// Which way the head is sliding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slide {
    /// Parked to shown.
    Reveal,
    /// Shown to parked.
    Retract,
}

/// Where and how opaque the content is at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Push to the right as a fraction of the window width: 0 is in place,
    /// 1 is fully past the right edge.
    pub offset: f32,
    /// Opacity, 0 (clear) to 1 (as drawn).
    pub opacity: f32,
}

impl Pose {
    /// Fully shown, in place.
    pub const SHOWN: Self = Self {
        offset: 0.0,
        opacity: 1.0,
    };

    /// The shift in pixels for a window `width` pixels wide.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // widths are far below 2^24
    pub fn shift_px(self, width: u32) -> u32 {
        let px = (self.offset.clamp(0.0, 1.0) * width as f32).round();
        // In range: clamped to [0, width] above.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = px as u32;
        px.min(width)
    }

    /// Opacity as a `0..=255` blend constant.
    #[must_use]
    pub fn alpha_u8(self) -> u8 {
        let a = (self.opacity.clamp(0.0, 1.0) * 255.0).round();
        // In range: clamped to [0, 255] above.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let a = a as u8;
        a
    }
}

impl Slide {
    /// Total length of this slide.
    #[must_use]
    pub fn duration(self, reduced_motion: bool) -> Duration {
        match (self, reduced_motion) {
            (_, true) => REDUCED_FADE,
            (Self::Reveal, false) => REVEAL,
            (Self::Retract, false) => RETRACT,
        }
    }

    /// The pose `elapsed` into the slide, or `None` once it has finished.
    #[must_use]
    pub fn pose(self, elapsed: Duration, reduced_motion: bool) -> Option<Pose> {
        let total = self.duration(reduced_motion);
        if elapsed >= total {
            return None;
        }
        let p = elapsed.as_secs_f32() / total.as_secs_f32();
        let pose = match (self, reduced_motion) {
            (Self::Reveal, true) => Pose {
                offset: 0.0,
                opacity: p,
            },
            (Self::Retract, true) => Pose {
                offset: 0.0,
                opacity: 1.0 - p,
            },
            (Self::Reveal, false) => {
                let q = 1.0 - (1.0 - p).powi(3);
                Pose {
                    offset: 1.0 - q,
                    opacity: q,
                }
            }
            (Self::Retract, false) => {
                let q = p.powi(3);
                Pose {
                    offset: q,
                    opacity: 1.0 - q,
                }
            }
        };
        Some(pose)
    }

    /// How far into this slide the head already is, when it starts from
    /// `from` (a pose of the opposite slide that was interrupted). The
    /// shown fraction is the opacity in every mode, so the head reverses
    /// from exactly where it is instead of jumping.
    #[must_use]
    pub fn resume_at(self, from: Pose, reduced_motion: bool) -> Duration {
        let shown = from.opacity.clamp(0.0, 1.0);
        let p = match (self, reduced_motion) {
            (Self::Reveal, true) => shown,
            (Self::Retract, true) => 1.0 - shown,
            // shown = 1 - (1 - p)³
            (Self::Reveal, false) => 1.0 - (1.0 - shown).cbrt(),
            // shown = 1 - p³
            (Self::Retract, false) => (1.0 - shown).cbrt(),
        };
        self.duration(reduced_motion).mul_f32(p.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_resumes_from_the_same_pose() {
        for reduced in [false, true] {
            for (from, to) in [
                (Slide::Reveal, Slide::Retract),
                (Slide::Retract, Slide::Reveal),
            ] {
                let total = from.duration(reduced);
                for k in 1..10 {
                    let pose = from.pose(total * k / 10, reduced).unwrap();
                    let resumed = to.pose(to.resume_at(pose, reduced), reduced).unwrap();
                    assert!(
                        (resumed.opacity - pose.opacity).abs() < 0.01
                            && (resumed.offset - pose.offset).abs() < 0.01,
                        "{from:?}->{to:?} at {k}/10 reduced={reduced}: {pose:?} vs {resumed:?}"
                    );
                }
            }
        }
        assert_eq!(
            Slide::Reveal.resume_at(Pose::SHOWN, false),
            REVEAL,
            "already shown"
        );
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn reveal_starts_off_screen_and_clear() {
        let p = Slide::Reveal.pose(Duration::ZERO, false).unwrap();
        assert_eq!(p.shift_px(168), 168);
        assert_eq!(p.alpha_u8(), 0);
    }

    #[test]
    fn retract_starts_in_place_and_opaque() {
        let p = Slide::Retract.pose(Duration::ZERO, false).unwrap();
        assert_eq!(p.shift_px(168), 0);
        assert_eq!(p.alpha_u8(), 255);
    }

    #[test]
    fn slides_finish_after_their_duration() {
        assert!(Slide::Reveal.pose(MS(259), false).is_some());
        assert!(Slide::Reveal.pose(MS(260), false).is_none());
        assert!(Slide::Retract.pose(MS(199), false).is_some());
        assert!(Slide::Retract.pose(MS(200), false).is_none());
        assert!(Slide::Reveal.pose(MS(60), true).is_none());
    }

    #[test]
    fn reveal_eases_out_and_retract_eases_in() {
        // Ease-out covers most of the distance in the first half…
        let reveal = Slide::Reveal.pose(MS(130), false).unwrap();
        assert!(reveal.offset < 0.2, "{reveal:?}");
        // …and ease-in has barely moved at the halfway point.
        let retract = Slide::Retract.pose(MS(100), false).unwrap();
        assert!(retract.offset < 0.2, "{retract:?}");
    }

    #[test]
    fn reveal_moves_monotonically_left() {
        let mut last = u32::MAX;
        for ms in (0..260).step_by(10) {
            let shift = Slide::Reveal.pose(MS(ms), false).unwrap().shift_px(168);
            assert!(shift <= last, "at {ms} ms");
            last = shift;
        }
    }

    #[test]
    fn reduced_motion_fades_in_place() {
        for slide in [Slide::Reveal, Slide::Retract] {
            let p = slide.pose(MS(30), true).unwrap();
            assert_eq!(p.shift_px(168), 0);
            assert!((p.opacity - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn conversions_clamp() {
        let wild = Pose {
            offset: 3.0,
            opacity: -1.0,
        };
        assert_eq!(wild.shift_px(100), 100);
        assert_eq!(wild.alpha_u8(), 0);
        assert_eq!(Pose::SHOWN.alpha_u8(), 255);
    }
}
