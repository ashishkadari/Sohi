//! Cutting a streamed reply into speakable sentences (spec §5.5).
//!
//! The reply arrives in pieces that split anywhere. [`SentenceChunker`]
//! hands out text as soon as a sentence is certain to be complete, so
//! speaking can start while the rest is still being written. Markdown is
//! stripped and code is never spoken.

/// Said once, in place of any code.
pub const DETAILS_NOTE: &str = "I've put the details in the text panel.";

/// Beyond this many characters a chunk may also end at `;`, `:` or `—`.
const CLAUSE_CHARS: usize = 60;
/// The first chunk may end at a comma from this many characters on.
const FIRST_COMMA_CHARS: usize = 40;
/// No chunk is longer than this.
const MAX_CHARS: usize = 200;
const FENCE: &str = "```";

/// Splits a streamed reply into chunks to speak.
#[derive(Debug, Default)]
pub struct SentenceChunker {
    /// Text not handed out yet, starting at the next chunk.
    pending: String,
    in_code: bool,
    started: bool,
    noted: bool,
}

impl SentenceChunker {
    /// A chunker at the start of a reply.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next piece; returns the chunks it completes, ready to
    /// speak.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        self.pending.push_str(delta);
        let mut out = Vec::new();
        while self.step(&mut out, false) {}
        out
    }

    /// Ends the reply; returns what is left to speak.
    pub fn flush(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while self.step(&mut out, true) {}
        out
    }

    /// Consumes one chunk or fence, if there is one.
    fn step(&mut self, out: &mut Vec<String>, last: bool) -> bool {
        let fence = self.pending.find(FENCE);
        if self.in_code {
            let Some(at) = fence else {
                // A fence split across two pieces may still complete.
                let keep = trailing_backticks(&self.pending);
                self.pending.drain(..self.pending.len() - keep);
                return false;
            };
            self.pending.drain(..at + FENCE.len());
            self.in_code = false;
            return true;
        }
        let prose_end =
            fence.unwrap_or_else(|| self.pending.len() - trailing_backticks(&self.pending));
        if let Some(end) = boundary(&self.pending[..prose_end], self.started) {
            self.emit(end, out);
            return true;
        }
        if fence.is_some() {
            self.emit(prose_end, out);
            self.pending.drain(..FENCE.len());
            self.in_code = true;
            if !self.noted {
                self.noted = true;
                self.started = true;
                out.push(DETAILS_NOTE.to_owned());
            }
            return true;
        }
        if last {
            self.emit(self.pending.len(), out);
        }
        false
    }

    /// Hands out the first `end` bytes, unless nothing in them is speakable.
    fn emit(&mut self, end: usize, out: &mut Vec<String>) {
        let text = strip_markdown(&self.pending[..end]);
        self.pending.drain(..end);
        if !text.is_empty() {
            self.started = true;
            out.push(text);
        }
    }
}

fn trailing_backticks(text: &str) -> usize {
    text.len() - text.trim_end_matches('`').len()
}

/// Where the first chunk of `text` ends, if that is already certain.
fn boundary(text: &str, started: bool) -> Option<usize> {
    let mut last_space = None;
    let mut words = false;
    for (count, (at, c)) in (1..).zip(text.char_indices()) {
        let end = at + c.len_utf8();
        if count > MAX_CHARS {
            return Some(last_space.unwrap_or(at));
        }
        if c == '\n' && words {
            return Some(end);
        }
        if c.is_whitespace() {
            last_space = Some(end);
        }
        let rest = &text[end..];
        let spaced = rest.starts_with(char::is_whitespace);
        let cut = match c {
            // "1." alone is a list number, not a sentence.
            '.' | '!' | '?' | '…' => words && ends_sentence(rest),
            ';' | ':' => count > CLAUSE_CHARS && spaced,
            '—' => count > CLAUSE_CHARS,
            ',' => !started && count >= FIRST_COMMA_CHARS && spaced,
            _ => false,
        };
        if cut {
            let closers = if c == '—' {
                0
            } else {
                rest.len() - rest.trim_start_matches(is_closer).len()
            };
            return Some(end + closers);
        }
        words |= c.is_alphabetic();
    }
    None
}

/// Whether the text after a `.`, `!`, `?` or `…` shows the sentence is over.
fn ends_sentence(rest: &str) -> bool {
    rest.trim_start_matches(is_closer)
        .starts_with(char::is_whitespace)
}

fn is_closer(c: char) -> bool {
    matches!(c, '"' | '\'' | '”' | '’' | ')' | ']' | '*' | '_' | '`')
}

/// `text` as it should be spoken: no markdown, no emoji, single spaces;
/// empty if there is nothing to say. A link becomes its text.
#[must_use]
pub fn strip_markdown(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    for line in text.lines() {
        strip_line(line_content(line), &mut plain);
        plain.push(' ');
    }
    if plain.contains(char::is_alphanumeric) {
        plain.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        String::new()
    }
}

/// A line without its heading, quote or bullet marker.
fn line_content(line: &str) -> &str {
    let line = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
    let heading = line.trim_start_matches('#');
    if heading.len() < line.len() && heading.starts_with(' ') {
        return heading;
    }
    ["- ", "* ", "+ ", "• "]
        .iter()
        .find_map(|bullet| line.strip_prefix(bullet))
        .unwrap_or(line)
}

/// Appends a line without its inline markup.
fn strip_line(line: &str, out: &mut String) {
    let mut rest = line;
    let mut before = ' ';
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        if c == '['
            && let Some((label, tail)) = link(after)
        {
            strip_line(label, out);
            rest = tail;
            before = ')';
            continue;
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            rest = &rest[url_len(rest)..];
            out.push_str("a link");
            before = 'k';
            continue;
        }
        if c == '|' {
            out.push(' ');
        } else if !is_markup(before, c, after) {
            out.push(c);
        }
        before = c;
        rest = after;
    }
}

/// Whether `c`, with `before` and `after` around it, is not to be spoken.
fn is_markup(before: char, c: char, after: &str) -> bool {
    match c {
        '`' => true,
        '*' => !(before.is_whitespace() && after.starts_with(char::is_whitespace)),
        '_' => !(before.is_alphanumeric() && after.starts_with(char::is_alphanumeric)),
        '~' => before == '~' || after.starts_with('~'),
        '!' => after.starts_with('['),
        _ => is_pictograph(c),
    }
}

/// Splits `label](target)rest` into the label and the rest.
fn link(text: &str) -> Option<(&str, &str)> {
    let (label, tail) = text.split_once("](")?;
    let (_, rest) = tail.split_once(')')?;
    Some((label, rest))
}

/// How long the URL at the start of `text` is, without the punctuation
/// that ends the sentence around it.
fn url_len(text: &str) -> usize {
    let end = text
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"'))
        .unwrap_or(text.len());
    text[..end]
        .trim_end_matches(['.', ',', ';', ':', '!', '?'])
        .len()
}

fn is_pictograph(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0xFE0F | 0x200D
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `text` in pieces of `size` characters.
    fn in_pieces(text: &str, size: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut chunker = SentenceChunker::new();
        let mut out = Vec::new();
        for piece in chars.chunks(size) {
            out.extend(chunker.push(&piece.iter().collect::<String>()));
        }
        out.extend(chunker.flush());
        out
    }

    /// The chunks of `text`, checking that how it arrives makes no
    /// difference.
    fn chunks(text: &str) -> Vec<String> {
        let whole = in_pieces(text, 100_000);
        for size in 1..=7 {
            assert_eq!(in_pieces(text, size), whole, "in pieces of {size}");
        }
        whole
    }

    #[test]
    fn splits_at_sentence_ends() {
        assert_eq!(
            chunks("It's noon. Are you hungry? Good… Go!"),
            ["It's noon.", "Are you hungry?", "Good…", "Go!"]
        );
        assert_eq!(
            chunks("\"Stop.\" (Really.) **Now.** Fine."),
            ["\"Stop.\"", "(Really.)", "Now.", "Fine."]
        );
    }

    #[test]
    fn numbers_and_names_with_dots_are_not_split() {
        assert_eq!(
            chunks("Version 3.14 of main.rs is out. Ok."),
            ["Version 3.14 of main.rs is out.", "Ok."]
        );
        assert_eq!(
            chunks("1. First thing. 2. Second."),
            ["1. First thing.", "2. Second."]
        );
    }

    #[test]
    fn a_sentence_is_handed_out_once_it_is_certain() {
        let mut chunker = SentenceChunker::new();
        assert_eq!(chunker.push("It is 3"), [""; 0]);
        assert_eq!(chunker.push("."), [""; 0]);
        assert_eq!(chunker.push("5 degrees."), [""; 0]);
        assert_eq!(chunker.push(" And"), ["It is 3.5 degrees."]);
        assert_eq!(chunker.flush(), ["And"]);
        assert_eq!(chunker.flush(), [""; 0]);
    }

    #[test]
    fn long_text_splits_at_clauses() {
        assert_eq!(
            chunks(
                "This clause goes on for rather more than sixty characters in all; then the rest"
            ),
            [
                "This clause goes on for rather more than sixty characters in all;",
                "then the rest"
            ]
        );
        assert_eq!(chunks("Short: no split; here"), ["Short: no split; here"]);
    }

    #[test]
    fn only_the_first_chunk_may_end_at_a_comma() {
        assert_eq!(
            chunks(
                "Since the weather is turning rather bad, you should stay in, as I said before, at home."
            ),
            [
                "Since the weather is turning rather bad,",
                "you should stay in, as I said before, at home."
            ]
        );
        assert_eq!(chunks("Well, short."), ["Well, short."]);
    }

    #[test]
    fn nothing_is_longer_than_the_cap() {
        for chunk in chunks(&"word ".repeat(100)) {
            assert!(chunk.chars().count() <= MAX_CHARS, "{}", chunk.len());
        }
        let solid = "é".repeat(450);
        let got = chunks(&solid);
        assert_eq!(got.concat(), solid);
        assert!(got.iter().all(|c| c.chars().count() <= MAX_CHARS));
    }

    #[test]
    fn code_is_replaced_by_one_note() {
        assert_eq!(
            chunks("Try this:\n```rust\nfn main() {}\n```\nOr this.\n```\nls\n```\nDone."),
            ["Try this:", DETAILS_NOTE, "Or this.", "Done."]
        );
        assert_eq!(chunks("Look: ```x. y. z"), ["Look:", DETAILS_NOTE]);
        assert_eq!(chunks("Run `cargo test` now."), ["Run cargo test now."]);
    }

    #[test]
    fn markdown_is_stripped() {
        for (text, plain) in [
            ("# Title", "Title"),
            ("> quoted _text_", "quoted text"),
            ("- **bold** and *soft*", "bold and soft"),
            ("see [the docs](https://x.y/z.) now", "see the docs now"),
            ("![a cat](c.png)", "a cat"),
            ("at https://example.com/a?b=c.", "at a link."),
            ("snake_case stays", "snake_case stays"),
            ("3 * 4 is 12", "3 * 4 is 12"),
            ("~~gone~~ but ~5", "gone but ~5"),
            ("Nice! 🎉", "Nice!"),
            ("| a | b |", "a b"),
            ("---", ""),
            ("***", ""),
        ] {
            assert_eq!(strip_markdown(text), plain, "{text:?}");
        }
    }

    #[test]
    fn lists_are_spoken_item_by_item() {
        assert_eq!(
            chunks("Two things:\n\n- *Eat*\n- Sleep\n"),
            ["Two things:", "Eat", "Sleep"]
        );
    }
}
