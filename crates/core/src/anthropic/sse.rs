//! Minimal server-sent-events line reader.
//!
//! Buffers **bytes**, not strings. HTTP chunk boundaries land mid-codepoint
//! often enough that decoding each chunk on arrival corrupts multi-byte
//! characters; splitting on `\n` first means every line is complete before it
//! is decoded.
//!
//! The `event:` line is ignored — every Anthropic frame repeats its type inside
//! the `data:` JSON, so switching on that one field is both simpler and the
//! thing the versioning policy asks for (unknown types must not break parsing).

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    /// Feed one HTTP chunk; returns the `data:` payloads completed by it.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();

        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();

            if let Some(rest) = line.strip_prefix("data:") {
                let payload = rest.trim();
                if !payload.is_empty() {
                    out.push(payload.to_string());
                }
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frames_and_ignores_event_lines() {
        let mut p = SseParser::default();
        let got = p.feed(b"event: ping\ndata: {\"type\":\"ping\"}\n\n");
        assert_eq!(got, vec![r#"{"type":"ping"}"#]);
    }

    #[test]
    fn holds_partial_line_until_newline() {
        let mut p = SseParser::default();
        assert!(p.feed(b"data: {\"ty").is_empty());
        assert_eq!(p.feed(b"pe\":\"ping\"}\n"), vec![r#"{"type":"ping"}"#]);
    }

    #[test]
    fn survives_a_codepoint_split_across_chunks() {
        // "é" is 0xC3 0xA9 — split it down the middle.
        let mut p = SseParser::default();
        assert!(p.feed(b"data: {\"t\":\"\xc3").is_empty());
        let got = p.feed(b"\xa9\"}\n");
        assert_eq!(got, vec!["{\"t\":\"\u{e9}\"}"]);
    }
}
