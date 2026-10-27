//! Incremental Server-Sent Events parser.
//!
//! The gateway needs two things at once: the decoded event fields for protocol
//! conversion, and the original bytes for same-protocol passthrough. Each frame
//! therefore carries both.

#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
    pub raw: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
    raw: Vec<u8>,
    has_fields: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buffer.extend_from_slice(chunk);

        let mut frames = Vec::new();
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line_end = position;
            if line_end > 0 && self.buffer[line_end - 1] == b'\r' {
                line_end -= 1;
            }
            let line = String::from_utf8_lossy(&self.buffer[..line_end]).to_string();
            let consumed = position + 1;
            self.raw.extend_from_slice(&self.buffer[..consumed]);
            self.buffer.drain(..consumed);
            self.process_line(&line, &mut frames);
        }

        frames
    }

    /// Flush a final frame when an upstream closes without a trailing blank line.
    pub fn finish(&mut self) -> Vec<SseFrame> {
        let mut frames = Vec::new();
        if !self.buffer.is_empty() {
            let mut remaining = std::mem::take(&mut self.buffer);
            if !remaining.ends_with(b"\n") {
                remaining.push(b'\n');
            }
            frames.extend(self.push(&remaining));
        }
        if self.has_fields {
            frames.push(SseFrame {
                event: self.event.take(),
                data: self.data.join("\n"),
                raw: std::mem::take(&mut self.raw),
            });
            self.data.clear();
            self.has_fields = false;
        }
        frames
    }

    fn process_line(&mut self, line: &str, frames: &mut Vec<SseFrame>) {
        if line.is_empty() {
            if self.has_fields {
                frames.push(SseFrame {
                    event: self.event.take(),
                    data: self.data.join("\n"),
                    raw: std::mem::take(&mut self.raw),
                });
                self.data.clear();
                self.has_fields = false;
            } else {
                self.raw.clear();
            }
            return;
        }

        if line.starts_with(':') {
            return;
        }

        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };

        match field {
            "event" => {
                self.event = Some(value.to_string());
                self.has_fields = true;
            }
            "data" => {
                self.data.push(value.to_string());
                self.has_fields = true;
            }
            _ => {
                // `id` and `retry` fields are kept in `raw` for passthrough.
                self.has_fields = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frames_split_across_chunks() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"event: content_block_delta\r\nda").is_empty());
        let frames = parser.push(b"ta: {\"type\":\"text_delta\"}\r\n\r\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(frames[0].data, "{\"type\":\"text_delta\"}");
        assert_eq!(
            frames[0].raw,
            b"event: content_block_delta\r\ndata: {\"type\":\"text_delta\"}\r\n\r\n"
        );
    }

    #[test]
    fn joins_multiline_data_and_ignores_comments() {
        let mut parser = SseParser::new();
        let frames = parser.push(b": keepalive\ndata: a\ndata: b\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event, None);
        assert_eq!(frames[0].data, "a\nb");
        assert!(frames[0].raw.starts_with(b": keepalive\n"));
    }

    #[test]
    fn finish_flushes_unterminated_frame() {
        let mut parser = SseParser::new();
        assert!(parser.push(b"data: [DONE]").is_empty());
        let frames = parser.finish();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "[DONE]");
    }
}
