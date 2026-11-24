//! Markdown as it streams in (spec §4.9): text in, styled pieces out.
//!
//! Only what models commonly write is understood: fenced code, headings,
//! lists, bold and inline code. Anything else stays plain text. Text is
//! held back only while it could still become a marker.

/// How a piece of text looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Ordinary text.
    Body,
    /// `**bold**`.
    Bold,
    /// A `#` heading.
    Heading,
    /// `` `code` `` in a line.
    InlineCode,
    /// Inside a code fence.
    Code,
    /// The language of a code fence, on a line of its own.
    Label,
}

/// A piece of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The text, with `\n` line breaks.
    pub text: String,
    /// Its style.
    pub style: Style,
}

/// Where the parser is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// At the start of a line outside code.
    LineStart,
    /// Within a line outside code.
    Line,
    /// On the line that opens a fence, after the backticks.
    FenceInfo,
    /// At the start of a line of code.
    CodeStart,
    /// Within a line of code.
    Code,
    /// On the line that closes a fence, after the backticks.
    FenceEnd,
}

/// The parser for one reply.
#[derive(Debug, Clone)]
pub struct Markdown {
    place: Place,
    /// Held back: it may become a marker.
    held: String,
    bold: bool,
    inline: bool,
    heading: bool,
    /// The code block being read.
    code: String,
    last_code: Option<String>,
    out: Vec<Span>,
}

impl Default for Markdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Markdown {
    /// A parser at the start of a reply.
    #[must_use]
    pub fn new() -> Self {
        Self {
            place: Place::LineStart,
            held: String::new(),
            bold: false,
            inline: false,
            heading: false,
            code: String::new(),
            last_code: None,
            out: Vec::new(),
        }
    }

    /// Takes the next piece of the reply.
    pub fn push(&mut self, text: &str) -> Vec<Span> {
        for c in text.chars().filter(|&c| c != '\r') {
            self.take(c);
        }
        std::mem::take(&mut self.out)
    }

    /// Ends the reply: what was held back is plain text after all, and a
    /// fence left open ends here.
    pub fn finish(&mut self) -> Vec<Span> {
        match self.place {
            Place::FenceInfo => self.held.clear(),
            Place::CodeStart if self.held.trim_start().starts_with("```") => {
                self.held.clear();
            }
            _ => self.release(),
        }
        if matches!(
            self.place,
            Place::CodeStart | Place::Code | Place::FenceInfo
        ) {
            self.close_code();
        }
        self.place = Place::LineStart;
        self.end_line_styles();
        std::mem::take(&mut self.out)
    }

    /// The latest code block, if there was one.
    #[must_use]
    pub fn last_code(&self) -> Option<&str> {
        self.last_code.as_deref()
    }

    fn take(&mut self, c: char) {
        match self.place {
            Place::LineStart => self.line_start(c),
            Place::Line => self.line(c),
            Place::FenceInfo if c == '\n' => {
                let label = self.held.trim().to_owned();
                self.held.clear();
                if !label.is_empty() {
                    self.emit(&label, Style::Label);
                    self.emit("\n", Style::Label);
                }
                self.place = Place::CodeStart;
            }
            Place::FenceInfo => self.held.push(c),
            Place::CodeStart => self.code_start(c),
            Place::Code => self.code_char(c),
            Place::FenceEnd => {
                if c == '\n' {
                    self.place = Place::LineStart;
                }
            }
        }
    }

    fn line_start(&mut self, c: char) {
        self.held.push(c);
        let marker = self.held.trim_start_matches(' ');
        if marker == "```" {
            self.held.clear();
            self.code.clear();
            self.place = Place::FenceInfo;
        } else if let Some(hashes) = marker.strip_suffix(' ')
            && (1..=6).contains(&hashes.len())
            && hashes.bytes().all(|b| b == b'#')
        {
            self.held.clear();
            self.heading = true;
            self.place = Place::Line;
        } else if matches!(marker, "- " | "* " | "+ ") {
            let indent = self.held.len() - marker.len();
            let bullet = format!("{}• ", " ".repeat(indent));
            self.held.clear();
            self.place = Place::Line;
            self.emit(&bullet, Style::Body);
        } else if !could_be_marker(marker) {
            let held = std::mem::take(&mut self.held);
            self.place = Place::Line;
            for c in held.chars() {
                self.line(c);
            }
        }
    }

    /// Ordinary text; `held` is at most one `*`.
    fn line(&mut self, c: char) {
        if c == '*' && !self.inline {
            if self.held.is_empty() {
                self.held.push(c);
            } else {
                self.held.clear();
                self.bold = !self.bold;
            }
            return;
        }
        self.release();
        match c {
            '\n' => {
                let style = self.style();
                self.emit("\n", style);
                self.end_line_styles();
                self.place = Place::LineStart;
            }
            '`' => self.inline = !self.inline,
            c => {
                let style = self.style();
                self.emit(c.encode_utf8(&mut [0; 4]), style);
            }
        }
    }

    fn code_start(&mut self, c: char) {
        self.held.push(c);
        let marker = self.held.trim_start_matches(' ');
        if marker == "```" {
            self.held.clear();
            self.close_code();
            self.place = Place::FenceEnd;
        } else if !"```".starts_with(marker) {
            let held = std::mem::take(&mut self.held);
            self.place = Place::Code;
            for c in held.chars() {
                self.code_char(c);
            }
        }
    }

    fn code_char(&mut self, c: char) {
        self.code.push(c);
        self.emit(c.encode_utf8(&mut [0; 4]), Style::Code);
        if c == '\n' {
            self.place = Place::CodeStart;
        }
    }

    fn close_code(&mut self) {
        let code = std::mem::take(&mut self.code);
        if !code.trim().is_empty() {
            self.last_code = Some(code);
        }
    }

    /// Writes out what was held back.
    fn release(&mut self) {
        if self.held.is_empty() {
            return;
        }
        let held = std::mem::take(&mut self.held);
        let style = if matches!(self.place, Place::CodeStart | Place::Code) {
            self.code.push_str(&held);
            Style::Code
        } else {
            self.style()
        };
        self.emit(&held, style);
    }

    fn end_line_styles(&mut self) {
        self.bold = false;
        self.inline = false;
        self.heading = false;
    }

    fn style(&self) -> Style {
        if self.inline {
            Style::InlineCode
        } else if self.heading {
            Style::Heading
        } else if self.bold {
            Style::Bold
        } else {
            Style::Body
        }
    }

    fn emit(&mut self, text: &str, style: Style) {
        match self.out.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.out.push(Span {
                text: text.to_owned(),
                style,
            }),
        }
    }
}

/// Whether more text could make `marker` a fence, heading or list marker.
fn could_be_marker(marker: &str) -> bool {
    "```".starts_with(marker)
        || matches!(marker, "-" | "*" | "+")
        || (marker.len() <= 6 && marker.bytes().all(|b| b == b'#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `text` in pieces of `size` characters, merging equal styles.
    fn parse(text: &str, size: usize) -> (Vec<(String, Style)>, Option<String>) {
        let mut md = Markdown::new();
        let chars: Vec<char> = text.chars().collect();
        let mut spans = Vec::new();
        for piece in chars.chunks(size) {
            spans.extend(md.push(&piece.iter().collect::<String>()));
        }
        spans.extend(md.finish());
        let mut merged: Vec<(String, Style)> = Vec::new();
        for s in spans {
            match merged.last_mut() {
                Some(last) if last.1 == s.style => last.0.push_str(&s.text),
                _ => merged.push((s.text, s.style)),
            }
        }
        (merged, md.last_code().map(str::to_owned))
    }

    /// The same, checking that the piece size makes no difference.
    fn spans(text: &str) -> Vec<(String, Style)> {
        let whole = parse(text, usize::MAX);
        for size in [1, 2, 3, 7] {
            assert_eq!(parse(text, size), whole, "pieces of {size}");
        }
        whole.0
    }

    fn s(text: &str, style: Style) -> (String, Style) {
        (text.to_owned(), style)
    }

    #[test]
    fn plain_text_is_body() {
        assert_eq!(spans("Hi.\n\nYou."), [s("Hi.\n\nYou.", Style::Body)]);
    }

    #[test]
    fn a_fence_is_code_with_a_label() {
        let text = "Here:\n```rust\nfn main() {\n    let x = 2 * 3;\n}\n```\nDone **now**.";
        assert_eq!(
            spans(text),
            [
                s("Here:\n", Style::Body),
                s("rust\n", Style::Label),
                s("fn main() {\n    let x = 2 * 3;\n}\n", Style::Code),
                s("Done ", Style::Body),
                s("now", Style::Bold),
                s(".", Style::Body),
            ]
        );
        let code = parse(text, 4).1;
        assert_eq!(
            code.as_deref(),
            Some("fn main() {\n    let x = 2 * 3;\n}\n")
        );
    }

    #[test]
    fn an_open_fence_ends_with_the_reply() {
        assert_eq!(spans("```\na"), [s("a", Style::Code)]);
        assert_eq!(parse("```\na\n``", 1).1.as_deref(), Some("a\n``"));
        assert_eq!(parse("```\na\n```", 1).1.as_deref(), Some("a\n"));
        assert_eq!(parse("```py", 1).1, None);
    }

    #[test]
    fn markdown_inside_code_is_code() {
        assert_eq!(
            spans("```\n# not a heading\n- x `y` **z**\n```"),
            [s("# not a heading\n- x `y` **z**\n", Style::Code)]
        );
    }

    #[test]
    fn headings_lists_and_inline_code() {
        assert_eq!(
            spans("## Plan\n- run `ls`\n  * two\n1. one"),
            [
                s("Plan\n", Style::Heading),
                s("• run ", Style::Body),
                s("ls", Style::InlineCode),
                s("\n  • two\n1. one", Style::Body),
            ]
        );
    }

    #[test]
    fn markers_that_are_not_stay() {
        for text in [
            "2 * 3 * 4",
            "#hash",
            "-5 degrees",
            "a * b",
            "*",
            "#",
            "####### x",
        ] {
            assert_eq!(spans(text), [s(text, Style::Body)], "{text}");
        }
        assert_eq!(spans("``"), [s("``", Style::Body)]);
    }

    #[test]
    fn styles_end_with_the_line() {
        assert_eq!(
            spans("**open\nnext `c\nend"),
            [
                s("open\n", Style::Bold),
                s("next ", Style::Body),
                s("c\n", Style::InlineCode),
                s("end", Style::Body),
            ]
        );
    }

    #[test]
    fn bold_at_the_start_of_a_line_is_not_a_list() {
        assert_eq!(
            spans("**Note** x"),
            [s("Note", Style::Bold), s(" x", Style::Body)]
        );
    }
}
