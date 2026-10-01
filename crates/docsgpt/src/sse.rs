//! Minimal server-sent-events parser: `data:` lines, comments, blank-line dispatch.

/// Incremental parser for a `text/event-stream` body.
///
/// Feed it raw chunks as they arrive with [`SseParser::push`]; it returns the
/// `data` payload of every event completed by that chunk. `id`, `event` and
/// `retry` fields are ignored, and comment lines (`: keepalive`) are skipped.
#[derive(Default, Debug)]
pub struct SseParser {
    /// Bytes of the current, not yet terminated line. Kept as bytes so a
    /// multi-byte character split across two chunks is decoded whole.
    buf: Vec<u8>,
    data: Vec<String>,
}

impl SseParser {
    /// Feed raw bytes; returns complete `data` payloads in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(rel) = self.buf[start..].iter().position(|&b| b == b'\n') {
            let end = start + rel;
            let raw = &self.buf[start..end];
            let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
            let line = String::from_utf8_lossy(raw).into_owned();
            start = end + 1;
            self.line(&line, &mut out);
        }
        self.buf.drain(..start);
        out
    }

    /// Flush a trailing event that has no final blank line.
    pub fn finish(&mut self) -> Option<String> {
        if !self.buf.is_empty() {
            let mut rest = std::mem::take(&mut self.buf);
            rest.push(b'\n');
            let _ = self.push(&rest);
        }
        if self.data.is_empty() {
            None
        } else {
            let d = self.data.join("\n");
            self.data.clear();
            Some(d)
        }
    }

    fn line(&mut self, line: &str, out: &mut Vec<String>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                out.push(self.data.join("\n"));
                self.data.clear();
            }
            return;
        }
        if line.starts_with(':') {
            return; // comment / keepalive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        if field == "data" {
            self.data.push(value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_across_chunks() {
        let mut p = SseParser::default();
        let mut got = p.push(b"id: 0\ndata: {\"a\":1}\n\n: keepalive\nid: 1\ndata: {\"b\"");
        assert_eq!(got, vec!["{\"a\":1}"]);
        got = p.push(b":2}\n\ndata: tail");
        assert_eq!(got, vec!["{\"b\":2}"]);
        assert_eq!(p.finish().as_deref(), Some("tail"));
    }

    #[test]
    fn keeps_multibyte_chars_split_across_chunks() {
        let mut p = SseParser::default();
        let bytes = "data: héllo ✓\n\n".as_bytes();
        // Split inside the two-byte "é".
        let cut = bytes.iter().position(|&b| b == 0xC3).unwrap() + 1;
        assert!(p.push(&bytes[..cut]).is_empty());
        assert_eq!(p.push(&bytes[cut..]), vec!["héllo ✓"]);
    }

    #[test]
    fn joins_multiline_data() {
        let mut p = SseParser::default();
        let got = p.push(b"data: a\r\ndata: b\r\n\r\n");
        assert_eq!(got, vec!["a\nb"]);
    }

    #[test]
    fn finish_without_pending_data_is_none() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: x\n\n").len() == 1);
        assert_eq!(p.finish(), None);
    }
}
