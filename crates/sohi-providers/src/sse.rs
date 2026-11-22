//! An incremental parser for server-sent events (the WHATWG event-stream
//! format), which the Mistral and Anthropic streaming APIs use.
//!
//! Bytes go in as they arrive from the network, split anywhere: in the
//! middle of a line, between `\r` and `\n`, or inside a multi-byte UTF-8
//! character. Lines are only decoded once complete, so a split character
//! is never mangled.

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, or empty for the default (`message`).
    pub event: String,
    /// The `data:` lines, joined with `\n`.
    pub data: String,
}

/// Parser state between chunks.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the line being read.
    line: Vec<u8>,
    /// The last chunk ended with `\r`; a leading `\n` belongs to it.
    skip_lf: bool,
    /// Whether the start of the stream (and a possible BOM) was handled.
    started: bool,
    event: String,
    data: String,
    has_data: bool,
}

impl SseParser {
    /// A parser at the start of a stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next bytes and returns every event they complete, in
    /// order. An event is complete at the blank line after it.
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.started {
            // Wait until a BOM can be recognised in full.
            let have = self.line.len() + bytes.len();
            if have < 3 && b"\xEF\xBB\xBF".starts_with(&[&self.line[..], bytes].concat()) {
                self.line.extend_from_slice(bytes);
                return events;
            }
            self.started = true;
            let mut head = std::mem::take(&mut self.line);
            head.extend_from_slice(bytes);
            let body = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&head);
            let body = body.to_vec();
            self.scan(&body, &mut events);
            return events;
        }
        if self.skip_lf {
            self.skip_lf = false;
            if let Some(rest) = bytes.strip_prefix(b"\n") {
                bytes = rest;
            }
        }
        self.scan(bytes, &mut events);
        events
    }

    fn scan(&mut self, mut bytes: &[u8], events: &mut Vec<SseEvent>) {
        while let Some(end) = bytes.iter().position(|&b| b == b'\n' || b == b'\r') {
            self.line.extend_from_slice(&bytes[..end]);
            let line = std::mem::take(&mut self.line);
            self.take_line(&line, events);
            let crlf = bytes[end] == b'\r';
            bytes = &bytes[end + 1..];
            if crlf {
                match bytes.first() {
                    Some(b'\n') => bytes = &bytes[1..],
                    Some(_) => {}
                    None => self.skip_lf = true,
                }
            }
        }
        self.line.extend_from_slice(bytes);
    }

    fn take_line(&mut self, line: &[u8], events: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(events);
            return;
        }
        if line[0] == b':' {
            return; // comment (keep-alive)
        }
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(colon) => {
                let value = &line[colon + 1..];
                (&line[..colon], value.strip_prefix(b" ").unwrap_or(value))
            }
            None => (line, &[][..]),
        };
        let value = String::from_utf8_lossy(value);
        match field {
            b"data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(&value);
                self.has_data = true;
            }
            b"event" => value.as_ref().clone_into(&mut self.event),
            _ => {} // `id`, `retry` and unknown fields are not used
        }
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        let event = std::mem::take(&mut self.event);
        if !std::mem::take(&mut self.has_data) {
            return;
        }
        events.push(SseEvent {
            event,
            data: std::mem::take(&mut self.data),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: event.to_owned(),
            data: data.to_owned(),
        }
    }

    fn parse_all(input: &[u8]) -> Vec<SseEvent> {
        SseParser::new().push(input)
    }

    #[test]
    fn data_lines_are_joined_and_dispatched_at_a_blank_line() {
        let got = parse_all(b"data: one\ndata: two\n\ndata: three\n\n");
        assert_eq!(got, [ev("", "one\ntwo"), ev("", "three")]);
    }

    #[test]
    fn event_names_comments_and_unknown_fields() {
        let input =
            b": keep-alive\nevent: content_block_delta\nid: 7\nretry: 10\nx: y\ndata: {}\n\n";
        assert_eq!(parse_all(input), [ev("content_block_delta", "{}")]);
    }

    #[test]
    fn every_line_ending_works() {
        let want = [ev("", "a"), ev("", "b"), ev("", "c")];
        assert_eq!(parse_all(b"data: a\r\n\r\ndata: b\r\rdata: c\n\n"), want);
    }

    #[test]
    fn only_one_leading_space_is_removed_and_no_colon_means_empty() {
        let got = parse_all(b"data:  two spaces\ndata\ndata:none\n\n");
        assert_eq!(got, [ev("", " two spaces\n\nnone")]);
    }

    #[test]
    fn events_without_data_are_not_dispatched() {
        assert_eq!(parse_all(b"event: ping\n\n: c\n\n"), []);
        // ...and do not leak their name into the next event.
        assert_eq!(parse_all(b"event: ping\n\ndata: x\n\n"), [ev("", "x")]);
    }

    #[test]
    fn an_unfinished_event_is_not_dispatched() {
        let mut parser = SseParser::new();
        assert_eq!(parser.push(b"data: partial\n"), []);
        assert_eq!(parser.push(b"\n"), [ev("", "partial")]);
    }

    #[test]
    fn a_leading_bom_is_skipped_even_when_split() {
        let mut parser = SseParser::new();
        assert_eq!(parser.push(b"\xEF"), []);
        assert_eq!(parser.push(b"\xBB"), []);
        assert_eq!(parser.push(b"\xBFdata: x\n\n"), [ev("", "x")]);
    }

    /// Every split point of a stream with CRLF, multi-byte UTF-8 and a
    /// mid-stream `\r`, fed as two chunks, gives the same events as one
    /// chunk. That covers `\r|\n` and in-character splits.
    #[test]
    fn splitting_anywhere_gives_the_same_events() {
        let input =
            "event: delta\r\ndata: héllo 👋 wörld\r\n\r\ndata: 日本語\rdata: second\r\r: bye\n"
                .as_bytes();
        let whole = parse_all(input);
        assert_eq!(
            whole,
            [ev("delta", "héllo 👋 wörld"), ev("", "日本語\nsecond")]
        );
        for split in 0..=input.len() {
            let mut parser = SseParser::new();
            let mut got = parser.push(&input[..split]);
            got.extend(parser.push(&input[split..]));
            assert_eq!(got, whole, "split at byte {split}");
        }
    }

    #[test]
    fn byte_by_byte_gives_the_same_events() {
        let input = "data: {\"choices\":[{\"delta\":{\"content\":\"Ça va\"}}]}\n\ndata: [DONE]\n\n";
        let mut parser = SseParser::new();
        let got: Vec<_> = input
            .as_bytes()
            .iter()
            .flat_map(|b| parser.push(std::slice::from_ref(b)))
            .collect();
        assert_eq!(got, parse_all(input.as_bytes()));
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].data, "[DONE]");
    }
}
