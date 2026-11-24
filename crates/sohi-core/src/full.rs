//! Full mode (spec §4.9): the phrases that open and close it, the
//! timelines of its two animations, and where things go in its window.

use std::time::Duration;

use crate::geometry::{RectPx, dip_to_px};

/// What a whole utterance asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open full mode.
    Enter,
    /// Close it.
    Leave,
}

/// Words that change nothing in a command. The name is among them, also
/// as it is misheard ("so he").
const FILLERS: [&str; 34] = [
    "um", "umm", "uh", "er", "hey", "hi", "hello", "ok", "okay", "yes", "sohi", "so", "he", "i",
    "soi", "sohee", "sophie", "please", "now", "go", "into", "to", "in", "the", "switch", "enter",
    "open", "start", "activate", "engage", "turn", "on", "screen", "core",
];
const LEAVING: [&str; 7] = ["exit", "leave", "close", "quit", "end", "stop", "off"];

/// Understands `text` if all of it is a full mode command. Filler words
/// and repeats are allowed; any other word makes it ordinary text.
#[must_use]
pub fn command(text: &str) -> Option<Command> {
    let plain: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .flat_map(char::to_lowercase)
        .collect();
    let mut leaving = false;
    let mut words = Vec::new();
    for word in plain.split_whitespace() {
        if LEAVING.contains(&word) {
            leaving = true;
        } else if matches!(word, "fullscreen" | "fullmode") {
            words.extend(["full", "mode"]);
        } else if !FILLERS.contains(&word) {
            words.push(word);
        }
    }
    // "fullscreen mode" reads as full, mode, mode.
    words.dedup();
    let pairs = words.chunks(2).all(|pair| pair == ["full", "mode"]);
    (!words.is_empty() && pairs).then_some(if leaving {
        Command::Leave
    } else {
        Command::Enter
    })
}

/// Which animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Take {
    /// The cape takes the screen over.
    Over,
    /// The cape is drawn back into the corner.
    Back,
}

/// One moment of an animation. Every value runs from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Moment {
    /// How far the head in the corner has turned towards the screen.
    pub turn: f32,
    /// How much of the screen the cape has swept.
    pub sweep: f32,
    /// The cape's opacity.
    pub cape: f32,
    /// The opacity of the head in the corner.
    pub head: f32,
    /// The opacity of the figure in the middle.
    pub figure: f32,
    /// How far the figure's eyes are open.
    pub eyes: f32,
    /// How far the figure has moved into the window's header.
    pub travel: f32,
}

impl Take {
    /// How long it takes.
    #[must_use]
    pub fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Self::Over => 2100,
            Self::Back => 800,
        })
    }

    /// When the cape hides everything behind it: the window is shown or
    /// hidden then.
    #[must_use]
    pub fn covered_at(self) -> Duration {
        Duration::from_millis(match self {
            Self::Over => 1280,
            Self::Back => 180,
        })
    }

    /// What to draw `at` this long after the start.
    #[must_use]
    pub fn moment(self, at: Duration) -> Moment {
        let ms = at.as_secs_f32() * 1000.0;
        match self {
            Self::Over => Moment {
                turn: ease(span(ms, 0.0, 380.0)),
                sweep: steady(span(ms, 240.0, 1260.0)),
                cape: 1.0 - ease(span(ms, 1720.0, 2100.0)),
                head: 1.0 - span(ms, 1050.0, 1150.0),
                figure: span(ms, 1200.0, 1430.0) * (1.0 - span(ms, 2020.0, 2100.0)),
                eyes: ease(span(ms, 1380.0, 1560.0)),
                travel: ease(span(ms, 1650.0, 2050.0)),
            },
            Self::Back => Moment {
                turn: 1.0 - ease(span(ms, 500.0, 780.0)),
                sweep: 1.0 - steady(span(ms, 200.0, 740.0)),
                cape: span(ms, 0.0, 170.0),
                head: span(ms, 250.0, 350.0),
                ..Moment::default()
            },
        }
    }
}

/// How far `ms` is between `from` and `to`.
fn span(ms: f32, from: f32, to: f32) -> f32 {
    ((ms - from) / (to - from)).clamp(0.0, 1.0)
}

/// Between [`ease`] and a constant speed.
fn steady(x: f32) -> f32 {
    f32::midpoint(x, ease(x))
}

/// Slow, fast, slow.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// The size of the head in the header (its layout grid's aspect).
const AVATAR_DIP: (u32, u32) = (168, 126);
const HEADER_DIP: u32 = 138;
const MARGIN_DIP: u32 = 32;
const COLUMN_DIP: u32 = 980;
const BUTTON_DIP: (u32, u32) = (104, 36);
const BUTTON_GAP_DIP: u32 = 10;
const INPUT_DIP: u32 = 104;
const INPUT_PAD_DIP: u32 = 12;
/// How many buttons the header has.
pub const BUTTONS: usize = 4;

/// Where everything in the window goes, in client pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layout {
    /// The head.
    pub avatar: RectPx,
    /// The name.
    pub title: RectPx,
    /// What Sohi is doing.
    pub status: RectPx,
    /// Talk, Copy code, New chat, Exit.
    pub buttons: [RectPx; BUTTONS],
    /// The line under the header.
    pub separator: RectPx,
    /// The conversation.
    pub transcript: RectPx,
    /// The frame around the input.
    pub input_frame: RectPx,
    /// The input.
    pub input: RectPx,
}

/// Lays out a window of `width` × `height` pixels at `dpi`. The
/// conversation is a centred column, no wider than reads well.
#[must_use]
pub fn layout(width: i32, height: i32, dpi: u32) -> Layout {
    let px = |dip: u32| dip_to_px(dip, dpi);
    let margin = px(MARGIN_DIP);
    let header = px(HEADER_DIP);
    let column = (width - 2 * margin).clamp(0, px(COLUMN_DIP));
    let left = (width - column) / 2;
    let rect = |left: i32, top: i32, w: i32, h: i32| RectPx {
        left,
        top,
        right: left + w.max(0),
        bottom: top + h.max(0),
    };
    let avatar = rect(left, px(6), px(AVATAR_DIP.0), px(AVATAR_DIP.1));
    let (bw, bh, gap) = (px(BUTTON_DIP.0), px(BUTTON_DIP.1), px(BUTTON_GAP_DIP));
    let mut buttons = [RectPx::default(); BUTTONS];
    let mut x = left + column;
    for button in buttons.iter_mut().rev() {
        x -= bw;
        *button = rect(x, (header - bh) / 2, bw, bh);
        x -= gap;
    }
    let text_x = avatar.right + px(8);
    let input_top = height - margin - px(INPUT_DIP);
    let pad = px(INPUT_PAD_DIP);
    let transcript_top = header + px(16);
    let transcript_bottom = input_top - px(16);
    Layout {
        avatar,
        title: rect(text_x, header / 2 - px(32), x - text_x, px(34)),
        status: rect(text_x, header / 2 + px(4), x - text_x, px(24)),
        buttons,
        separator: rect(left, header, column, 1),
        transcript: rect(
            left,
            transcript_top,
            column,
            transcript_bottom - transcript_top,
        ),
        input_frame: rect(left, input_top, column, px(INPUT_DIP)),
        input: rect(
            left + pad,
            input_top + pad,
            column - 2 * pad,
            px(INPUT_DIP) - 2 * pad,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_utterances_are_commands() {
        for text in [
            "full mode",
            "Full mode.",
            "Full mode, um, full mode.",
            "Hey Sohi, full mode please",
            "So he, go into full mode!",
            "Full-mode",
            "fullscreen mode",
            "full screen mode",
            "full core mode",
        ] {
            assert_eq!(command(text), Some(Command::Enter), "{text}");
        }
        for text in ["exit full mode", "Sohi, close full mode.", "full mode off"] {
            assert_eq!(command(text), Some(Command::Leave), "{text}");
        }
    }

    #[test]
    fn sentences_about_it_are_not() {
        for text in [
            "",
            "Sohi",
            "full",
            "mode",
            "mode full",
            "what is full mode",
            "how do I open full mode",
            "exit",
            "full screen the video",
            "full mode and play music",
            "don't open full mode",
        ] {
            assert_eq!(command(text), None, "{text}");
        }
    }

    #[test]
    fn the_cape_hides_the_screen_when_the_window_changes() {
        for take in [Take::Over, Take::Back] {
            let m = take.moment(take.covered_at());
            assert!(m.sweep >= 1.0 && m.cape >= 1.0, "{take:?}: {m:?}");
            assert!(take.covered_at() < take.duration());
        }
    }

    #[test]
    fn animations_start_and_end_clean() {
        let zero = |v: f32| v.abs() < 1e-3;
        let over = Take::Over.moment(Duration::ZERO);
        assert!(zero(over.turn) && zero(over.sweep) && zero(over.figure));
        let over = Take::Over.moment(Take::Over.duration());
        assert!(zero(over.cape) && zero(over.figure) && zero(over.head));
        let back = Take::Back.moment(Take::Back.duration());
        assert!(zero(back.sweep) && zero(back.turn) && zero(back.head - 1.0));
    }

    #[test]
    fn nothing_jumps_between_frames() {
        for take in [Take::Over, Take::Back] {
            let mut last = take.moment(Duration::ZERO);
            for ms in (8..=take.duration().as_millis()).step_by(8) {
                let m = take.moment(Duration::from_millis(u64::try_from(ms).unwrap()));
                assert!((m.sweep - last.sweep).abs() < 0.05);
                assert!((m.travel - last.travel).abs() < 0.05);
                last = m;
            }
        }
    }

    #[test]
    fn the_window_is_laid_out_without_overlaps() {
        for (w, h, dpi) in [(1280, 800, 96), (1920, 1200, 144), (3840, 2160, 192)] {
            let l = layout(w, h, dpi);
            assert!(l.avatar.right < l.title.left);
            assert!(l.title.right <= l.buttons[0].left);
            assert!(l.title.width() > 100, "{l:?}");
            for pair in l.buttons.windows(2) {
                assert!(pair[0].right < pair[1].left);
            }
            assert!(l.avatar.bottom <= l.separator.top);
            assert!(l.separator.bottom < l.transcript.top);
            assert!(l.transcript.bottom < l.input_frame.top);
            assert!(l.transcript.height() > 200);
            assert!(l.input_frame.bottom < h);
        }
    }

    #[test]
    fn a_tiny_window_is_no_panic() {
        let l = layout(0, 0, 96);
        assert_eq!(l.transcript.height(), 0);
    }
}
