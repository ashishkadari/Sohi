//! Text pill placement and layout (spec §4.1, §4.6). Pure: callers pass
//! the head's rectangle, the DPI and the font's line height.

use crate::geometry::{BASE_DPI, HEAD_SCALE_PERCENT, RectPx, dip_to_px};

/// The input row: 320 × 36 DIP.
pub const PILL_SIZE_DIP: (u32, u32) = (320, 36);
/// Gap between the head's frame and the pill.
pub const PILL_GAP_DIP: u32 = 8;
/// Left and right padding inside the pill.
pub const PILL_PAD_DIP: u32 = 12;
/// Room for the `›` prompt before the input.
pub const PROMPT_DIP: u32 = 14;
/// Padding above and below the bubble.
pub const BUBBLE_PAD_DIP: u32 = 6;
/// The bubble grows to this many lines, then scrolls.
pub const BUBBLE_MAX_LINES: u32 = 6;
/// The head's frame on the layout grid: left, top, width, height. Must
/// match the renderer's `FRAME`.
pub const HEAD_FRAME_LAYOUT: (u32, u32, u32, u32) = (12, 10, 88, 64);

/// Scales a length on the head's layout grid to physical pixels, the way
/// the renderer draws it.
fn layout_to_px(units: u32, dpi: u32) -> i32 {
    let num = u64::from(units) * u64::from(HEAD_SCALE_PERCENT) * u64::from(dpi);
    let den = 100 * u64::from(BASE_DPI);
    i32::try_from((num + den / 2) / den).unwrap_or(i32::MAX)
}

/// Where the pill's top-left corner goes, and how wide it is: 8 DIP below
/// the head's frame, right-aligned with it.
#[must_use]
pub fn pill_origin(head: RectPx, dpi: u32) -> (i32, i32, i32) {
    let (left, top, width, height) = HEAD_FRAME_LAYOUT;
    let frame_right = head.left + layout_to_px(left + width, dpi);
    let frame_bottom = head.top + layout_to_px(top + height, dpi);
    let pill_width = dip_to_px(PILL_SIZE_DIP.0, dpi);
    (
        frame_right - pill_width,
        frame_bottom + dip_to_px(PILL_GAP_DIP, dpi),
        pill_width,
    )
}

/// Where everything inside the pill goes, in client pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PillLayout {
    /// Client width.
    pub width: i32,
    /// Client height: the input row, plus the bubble if it has text.
    pub height: i32,
    /// Left edge of the `›` prompt.
    pub prompt_x: i32,
    /// The single-line input.
    pub input: RectPx,
    /// The 1 px line between input and bubble, if the bubble shows.
    pub separator: Option<RectPx>,
    /// The bubble, if it has text.
    pub bubble: Option<RectPx>,
    /// Whether the bubble has more lines than fit, so it scrolls.
    pub scrolls: bool,
}

/// Lays out a pill at `dpi` whose font is `line_px` tall, with
/// `bubble_lines` lines of reply text (0 hides the bubble).
#[must_use]
pub fn pill_layout(dpi: u32, line_px: i32, bubble_lines: u32) -> PillLayout {
    let px = |dip| dip_to_px(dip, dpi);
    let width = px(PILL_SIZE_DIP.0);
    let row = px(PILL_SIZE_DIP.1);
    let pad = px(PILL_PAD_DIP);
    let input_height = line_px + 2;
    let input_top = (row - input_height) / 2;
    let input = RectPx {
        left: pad + px(PROMPT_DIP),
        top: input_top,
        right: width - pad,
        bottom: input_top + input_height,
    };
    let mut layout = PillLayout {
        width,
        height: row,
        prompt_x: pad,
        input,
        separator: None,
        bubble: None,
        scrolls: false,
    };
    if bubble_lines == 0 {
        return layout;
    }
    let shown = i32::try_from(bubble_lines.min(BUBBLE_MAX_LINES)).unwrap_or(0);
    let bubble_top = row + 1 + px(BUBBLE_PAD_DIP);
    let bubble = RectPx {
        left: pad,
        top: bubble_top,
        right: width - pad / 2,
        bottom: bubble_top + shown * line_px,
    };
    layout.separator = Some(RectPx {
        left: pad,
        top: row,
        right: width - pad,
        bottom: row + 1,
    });
    layout.height = bubble.bottom + px(BUBBLE_PAD_DIP);
    layout.bubble = Some(bubble);
    layout.scrolls = bubble_lines > BUBBLE_MAX_LINES;
    layout
}

/// Converts reply text for a Win32 EDIT control: line breaks become
/// `\r\n` (a lone `\n` does not break the line there) and stray `\r`
/// are dropped, so text streamed in pieces converts the same either way.
#[must_use]
pub fn edit_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' => out.push_str("\r\n"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::head_window_rect;

    const FHD: RectPx = RectPx {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn pill_sits_under_the_frame_right_aligned() {
        let head = head_window_rect(FHD, 96);
        let (x, y, w) = pill_origin(head, 96);
        assert_eq!(w, 320);
        // Frame right edge: (12 + 88) × 1.25 = 125 DIP into the head window.
        assert_eq!(x + w, head.left + 125);
        // Frame bottom: (10 + 64) × 1.25 = 92.5 → 93, plus the 8 DIP gap.
        assert_eq!(y, head.top + 93 + 8);
        assert!(x >= FHD.left, "pill must stay on screen");
    }

    #[test]
    fn pill_scales_with_dpi() {
        let head = head_window_rect(FHD, 144);
        let (x, y, w) = pill_origin(head, 144);
        assert_eq!(w, 480);
        assert_eq!(x + w, head.left + 188); // 125 DIP × 1.5 = 187.5 → 188
        assert_eq!(y, head.top + 139 + 12);
    }

    #[test]
    fn an_empty_bubble_is_hidden() {
        let l = pill_layout(96, 18, 0);
        assert_eq!((l.width, l.height), (320, 36));
        assert_eq!(l.bubble, None);
        assert_eq!(l.separator, None);
        assert_eq!(l.input.left, 12 + 14);
        assert_eq!(l.input.right, 320 - 12);
        // Centred in the 36 px row.
        assert_eq!(l.input.top, (36 - 20) / 2);
    }

    #[test]
    fn the_bubble_grows_to_six_lines_then_scrolls() {
        let line = 18;
        let heights: Vec<_> = (1..=8).map(|n| pill_layout(96, line, n).height).collect();
        for pair in heights.windows(2).take(5) {
            assert_eq!(pair[1] - pair[0], line);
        }
        assert_eq!(heights[5], heights[6]);
        assert_eq!(heights[6], heights[7]);
        assert!(!pill_layout(96, line, 6).scrolls);
        assert!(pill_layout(96, line, 7).scrolls);
        let l = pill_layout(96, line, 2);
        let bubble = l.bubble.unwrap();
        assert_eq!(bubble.height(), 2 * 18);
        assert_eq!(l.height, bubble.bottom + 6);
        assert!(l.separator.unwrap().top >= l.input.bottom);
    }

    #[test]
    fn edit_text_normalises_line_breaks() {
        assert_eq!(edit_text("a\nb\r\nc\rd"), "a\r\nb\r\ncd");
        // A \r\n split across two pieces converts the same as whole.
        assert_eq!(edit_text("x\r") + &edit_text("\ny"), edit_text("x\r\ny"));
    }
}
