//! Pure renderer: turns a visual state and time into a premultiplied pixmap.
//!
//! [`HeadRenderer`] draws the head (spec §4.5) and [`CapeRenderer`] the
//! full mode animations (§4.9). [`test_card`] is a neutral
//! frame kept for plumbing checks (`--dev-test-card`), and [`tray_icon`]
//! draws the notification-area icon.

#![forbid(unsafe_code)]

mod cape;
mod head;

pub use cape::CapeRenderer;
pub use head::{HeadRenderer, Rendered, Scene};

/// A neutral, deliberately non-final frame for plumbing tests.
///
/// RGBA, premultiplied, `width * height * 4` bytes. It has a fully
/// transparent outer margin (to test click-through), a translucent grey
/// panel, and a 2 px light outline. `level` (0–255) scales the panel's
/// brightness so animation can be checked by eye.
#[must_use]
pub fn test_card(width: u32, height: u32, level: u8) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![0u8; w * h * 4];
    let margin_x = w / 6;
    let margin_y = h / 6;
    let outline = 2;
    for y in 0..h {
        for x in 0..w {
            let inside = x >= margin_x && x < w - margin_x && y >= margin_y && y < h - margin_y;
            if !inside {
                continue;
            }
            let edge = x < margin_x + outline
                || x >= w - margin_x - outline
                || y < margin_y + outline
                || y >= h - margin_y - outline;
            let (grey, alpha) = if edge {
                (200u16, 255u16)
            } else {
                (40 + u16::from(level) * 80 / 255, 200)
            };
            let premul = u8::try_from(grey * alpha / 255).unwrap_or(u8::MAX);
            let i = (y * w + x) * 4;
            out[i..i + 4].copy_from_slice(&[
                premul,
                premul,
                premul,
                u8::try_from(alpha).unwrap_or(255),
            ]);
        }
    }
    out
}

/// Sohi's red accent (`RED_ACTIVE`, spec §4.5).
const RED: [u8; 3] = [0xB0, 0x12, 0x2A];

/// The tray icon (spec §4.8): an opaque red square on a clear background,
/// `size` x `size` RGBA, inset by an eighth of the size on each side.
#[must_use]
pub fn tray_icon(size: u32) -> Vec<u8> {
    let n = size as usize;
    let inset = n / 8;
    let mut out = vec![0u8; n * n * 4];
    for y in inset..n - inset {
        for x in inset..n - inset {
            let i = (y * n + x) * 4;
            out[i..i + 4].copy_from_slice(&[RED[0], RED[1], RED[2], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn tray_icon_is_inset_red_square() {
        for size in [16, 20, 24, 32] {
            let buf = tray_icon(size);
            assert_eq!(buf.len(), (size * size * 4) as usize);
            assert_eq!(pixel(&buf, size, 0, 0), [0, 0, 0, 0]);
            assert_eq!(
                pixel(&buf, size, size / 2, size / 2),
                [0xB0, 0x12, 0x2A, 255]
            );
        }
    }

    #[test]
    fn size_and_transparent_margin() {
        let buf = test_card(168, 126, 0);
        assert_eq!(buf.len(), 168 * 126 * 4);
        assert_eq!(pixel(&buf, 168, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&buf, 168, 167, 125), [0, 0, 0, 0]);
    }

    #[test]
    fn centre_is_translucent_and_outline_opaque() {
        let buf = test_card(168, 126, 255);
        assert_eq!(pixel(&buf, 168, 84, 63)[3], 200);
        assert_eq!(pixel(&buf, 168, 28, 63)[3], 255);
    }

    #[test]
    fn colour_never_exceeds_alpha_premultiplied() {
        for level in [0, 128, 255] {
            let buf = test_card(112, 84, level);
            for px in buf.as_chunks::<4>().0 {
                assert!(px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]);
            }
        }
    }

    #[test]
    fn level_changes_brightness() {
        let dark = test_card(112, 84, 0);
        let bright = test_card(112, 84, 255);
        assert!(pixel(&bright, 112, 56, 42)[0] > pixel(&dark, 112, 56, 42)[0]);
    }
}
