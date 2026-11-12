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
        // Buffered bytes were already scanned by the previous feed.
        let scan_start = self.buf.len();
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut line_start = 0;

        for index in scan_start..self.buf.len() {
            if self.buf[index] != b'\n' {
                continue;
            }
            let line = String::from_utf8_lossy(&self.buf[line_start..index]);
            let line = line.trim();

            if let Some(rest) = line.strip_prefix("data:") {
                let payload = rest.trim();
                if !payload.is_empty() {
                    out.push(payload.to_string());
                }
            }
            line_start = index + 1;
        }
        if line_start > 0 {
            // Move the unfinished tail only once, regardless of the line count.
            self.buf.drain(..line_start);
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

    #[test]
    fn chunk_boundaries_preserve_payloads_and_the_unfinished_tail() {
        let stream = b": keepalive\r\nevent: update\r\ndata: \xc3\xa9\r\n\r\ndata:\ndata: \xff\ndata: [DONE]\n\ndata: tail";
        for chunk_size in 1..=stream.len() {
            let mut parser = SseParser::default();
            let got: Vec<_> = stream
                .chunks(chunk_size)
                .flat_map(|chunk| parser.feed(chunk))
                .collect();
            assert_eq!(got, ["é", "�", "[DONE]"], "chunk size {chunk_size}");
            assert!(parser.feed(b"").is_empty());
            assert_eq!(parser.feed(b" end\r\n"), ["tail end"]);
        }
    }
}
