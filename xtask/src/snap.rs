//! `cargo xtask snap [--out DIR]`: renders every visual state at 100 %,
//! 150 % and 200 % scaling (spec §8 P1) straight from `sohi-render`, with
//! no window involved. Writes one transparent PNG per state and scale, and
//! `sheet.png`, a contact sheet on a dark desktop-like background.
//!
//! Default output: `target/snap/`.

use std::path::PathBuf;
use std::time::Duration;

use sohi_core::geometry::{BASE_DPI, HEAD_WINDOW_DIP, dip_to_px};
use sohi_core::visual::VisualState;
use sohi_render::{HeadRenderer, Scene};

use crate::{png, root};

/// Display scales to render, as effective DPI.
const DPIS: [u32; 3] = [96, 144, 192];
/// Contact sheet background (a dark desktop) and the gap between cells.
const SHEET_BG: [u8; 3] = [0x1B, 0x1D, 0x22];
const GAP: u32 = 12;

/// One snapshot: a name and the scene that shows it best.
struct Shot {
    name: &'static str,
    scene: Scene,
}

/// Every state at a moment where its animation is visible, plus the error
/// flash over Idle.
fn shots() -> Vec<Shot> {
    let mut shots: Vec<Shot> = VisualState::ALL
        .into_iter()
        .map(|state| {
            let (t, level) = match state {
                VisualState::Listening | VisualState::Speaking => (Duration::ZERO, 0.7),
                VisualState::Transcribing => (Duration::from_millis(450), 0.0),
                VisualState::Thinking => (Duration::from_millis(200), 0.0),
                _ => (Duration::ZERO, 0.0),
            };
            Shot {
                name: state.name(),
                scene: Scene {
                    level,
                    ..Scene::new(state, t)
                },
            }
        })
        .collect();
    shots.push(Shot {
        name: "error",
        scene: Scene {
            error_age: Some(Duration::ZERO),
            ..Scene::new(VisualState::Idle, Duration::ZERO)
        },
    });
    shots
}

/// Runs the command.
pub fn run(args: &[String]) -> Result<(), String> {
    let out = match args {
        [] => root().join("target").join("snap"),
        [flag, dir] if flag == "--out" => PathBuf::from(dir),
        _ => return Err("usage: cargo xtask snap [--out DIR]".to_owned()),
    };
    std::fs::create_dir_all(&out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let shots = shots();
    let sizes: Vec<(u32, u32)> = DPIS
        .iter()
        .map(|&dpi| {
            let px = |dip: u32| u32::try_from(dip_to_px(dip, dpi)).unwrap_or(0);
            (px(HEAD_WINDOW_DIP.0), px(HEAD_WINDOW_DIP.1))
        })
        .collect();
    let mut sheet = Sheet::new(&sizes, shots.len());
    for (col, (&dpi, &(w, h))) in DPIS.iter().zip(&sizes).enumerate() {
        let mut renderer =
            HeadRenderer::new(w, h).ok_or_else(|| format!("cannot render at {w}x{h}"))?;
        for (row, shot) in shots.iter().enumerate() {
            let rgba = renderer.render(&shot.scene).rgba.to_vec();
            let straight = unpremultiply(&rgba);
            let file = out.join(format!("{}@{}.png", shot.name, dpi * 100 / BASE_DPI));
            std::fs::write(&file, png::encode(w, h, &straight))
                .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
            sheet.place(col, row, w, &rgba);
        }
    }
    let file = out.join("sheet.png");
    std::fs::write(&file, png::encode(sheet.width, sheet.height, &sheet.pixels))
        .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    eprintln!(
        "xtask: wrote {} snapshots and sheet.png to {}",
        shots.len() * DPIS.len(),
        out.display()
    );
    Ok(())
}

/// Premultiplied RGBA (renderer output) to straight RGBA (PNG).
fn unpremultiply(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let alpha = u16::from(a);
            let un = |c: u8| {
                (u16::from(c) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .map_or(0, |v| u8::try_from(v).unwrap_or(255))
            };
            [un(r), un(g), un(b), a]
        })
        .collect()
}

/// A grid: one column per scale, one row per shot, on a dark background.
struct Sheet {
    width: u32,
    height: u32,
    /// Left edge of each column and top edge of each row.
    cols: Vec<u32>,
    rows: Vec<u32>,
    pixels: Vec<u8>,
}

impl Sheet {
    fn new(sizes: &[(u32, u32)], rows: usize) -> Self {
        let mut cols = Vec::new();
        let mut x = GAP;
        for &(w, _) in sizes {
            cols.push(x);
            x += w + GAP;
        }
        let row_h = sizes.iter().map(|&(_, h)| h).max().unwrap_or(0) + GAP;
        let rows: Vec<u32> = (0..u32::try_from(rows).unwrap_or(0))
            .map(|r| GAP + r * row_h)
            .collect();
        let height = GAP + row_h * u32::try_from(rows.len()).unwrap_or(0);
        let pixels = [SHEET_BG[0], SHEET_BG[1], SHEET_BG[2], 255].repeat((x * height) as usize);
        Self {
            width: x,
            height,
            cols,
            rows,
            pixels,
        }
    }

    /// Composites premultiplied `rgba` (`w` wide) over the background.
    fn place(&mut self, col: usize, row: usize, w: u32, rgba: &[u8]) {
        let (x0, y0) = (self.cols[col] as usize, self.rows[row] as usize);
        let (w, sheet_w) = (w as usize, self.width as usize);
        for (i, src) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (x0 + i % w, y0 + i / w);
            let d = (y * sheet_w + x) * 4;
            let inv = 255 - u16::from(src[3]);
            for c in 0..3 {
                let over = u16::from(src[c]) + (u16::from(self.pixels[d + c]) * inv + 127) / 255;
                self.pixels[d + c] = u8::try_from(over).unwrap_or(255);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_restores_colour() {
        assert_eq!(unpremultiply(&[64, 32, 0, 128]), vec![128, 64, 0, 128]);
        assert_eq!(unpremultiply(&[0, 0, 0, 0]), vec![0, 0, 0, 0]);
        assert_eq!(unpremultiply(&[10, 20, 30, 255]), vec![10, 20, 30, 255]);
    }

    #[test]
    fn every_state_and_the_error_flash_are_shot() {
        let names: Vec<_> = shots().iter().map(|s| s.name).collect();
        assert_eq!(names.len(), VisualState::ALL.len() + 1);
        assert!(names.contains(&"forced-offline") && names.contains(&"error"));
    }
}
