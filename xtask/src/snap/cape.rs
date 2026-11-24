//! Frames of the full mode animations (spec §4.9), as one sheet each.

use std::path::Path;
use std::time::{Duration, Instant};

use sohi_core::full::{Take, layout};
use sohi_core::geometry::{RectPx, head_window_rect};
use sohi_render::CapeRenderer;

use super::SHEET_BG;
use crate::png;

/// The screen that is pretended, halved as on a real one.
const SCREEN: (i32, i32) = (1920, 1200);
const DPI: u32 = 144;
const COLUMNS: u32 = 4;
const FRAMES: u32 = 12;
/// Every how many pixels one is kept.
const SKIP: u32 = 2;

/// Writes `cape-over.png` and `cape-back.png`, and prints what frames
/// cost.
pub fn sheets(out: &Path) -> Result<(), String> {
    for (take, name) in [(Take::Over, "over"), (Take::Back, "back")] {
        let file = out.join(format!("cape-{name}.png"));
        let (w, h, pixels) = sheet(take).ok_or("cannot render the cape")?;
        std::fs::write(&file, png::encode(w, h, &pixels))
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }
    Ok(())
}

fn half(r: RectPx) -> RectPx {
    RectPx {
        left: r.left / 2,
        top: r.top / 2,
        right: r.right / 2,
        bottom: r.bottom / 2,
    }
}

fn sheet(take: Take) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (SCREEN.0.unsigned_abs() / 2, SCREEN.1.unsigned_abs() / 2);
    let work = RectPx {
        right: SCREEN.0,
        bottom: SCREEN.1,
        ..RectPx::default()
    };
    let head = half(head_window_rect(work, DPI));
    let avatar = half(layout(SCREEN.0, SCREEN.1, DPI).avatar);
    let mut cape = CapeRenderer::new(w, h, head, avatar)?;
    let (cw, ch) = (w / SKIP, h / SKIP);
    let (sw, sh) = (cw * COLUMNS, ch * FRAMES.div_ceil(COLUMNS));
    let mut pixels = [SHEET_BG[0], SHEET_BG[1], SHEET_BG[2], 255].repeat((sw * sh) as usize);
    let mut worst = Duration::ZERO;
    for n in 0..FRAMES {
        let at = take.duration() * n / (FRAMES - 1);
        let moment = take.moment(at);
        let started = Instant::now();
        let frame = cape.render(&moment, at);
        worst = worst.max(started.elapsed());
        let (ox, oy) = (n % COLUMNS * cw, n / COLUMNS * ch);
        for y in 0..ch {
            for x in 0..cw {
                let from = ((y * SKIP * w + x * SKIP) * 4) as usize;
                let to = (((oy + y) * sw + ox + x) * 4) as usize;
                let (Some(src), Some(dst)) =
                    (frame.get(from..from + 4), pixels.get_mut(to..to + 3))
                else {
                    continue;
                };
                let rest = 255 - u16::from(src[3]);
                for (d, &s) in dst.iter_mut().zip(src) {
                    let under = u16::from(*d) * rest / 255;
                    *d = u8::try_from(u16::from(s) + under).unwrap_or(255);
                }
            }
        }
    }
    eprintln!("xtask: cape {take:?}: slowest frame {worst:?}");
    Some((sw, sh, pixels))
}
