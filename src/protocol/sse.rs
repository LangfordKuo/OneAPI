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

        // Detach the buffer so line slices can be inspected while `self` is
        // mutated by `process_line`; it is put back (compacted) at the end.
        let mut buffer = std::mem::take(&mut self.buffer);
        let mut frames = Vec::new();
        let mut start = 0usize;
        while start < buffer.len() {
            // LF, CRLF and bare CR all terminate a line. A CR in the last byte
            // may be the first half of a CRLF split across chunks, so it stays
            // in the buffer until the next push or `finish`.
            let Some(relative) = buffer[start..]
                .iter()
                .position(|byte| *byte == b'\n' || *byte == b'\r')
            else {
                break;
            };
            let position = start + relative;
            let line_end = position;
            let mut next = position + 1;
            if buffer[position] == b'\r' {
                if next == buffer.len() {
                    break;
                }
                if buffer[next] == b'\n' {
                    next += 1;
                }
            }
            self.raw.extend_from_slice(&buffer[start..next]);
            self.process_line(&buffer[start..line_end], &mut frames);
            start = next;
        }

        // Compact once per chunk instead of draining the front for every line.
        if start > 0 {
            buffer.drain(..start);
        }
        self.buffer = buffer;

        frames
    }

    /// Bytes of the frame that is still being received. This includes the
    /// complete lines already parsed into `data`/`raw`, not just the current
    /// partial line: an upstream can otherwise pin memory by sending an endless
    /// run of `data:` lines without the terminating blank line.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len() + self.raw.len()
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

    fn process_line(&mut self, line: &[u8], frames: &mut Vec<SseFrame>) {
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

        if line[0] == b':' {
            return;
        }

        // Work on bytes: most lines (`id:`, `retry:`, blank separators) never
        // need a String, and only `event`/`data` values pay for UTF-8 decoding.
        let (field, value) = match line.iter().position(|byte| *byte == b':') {
            Some(index) => {
                let value = &line[index + 1..];
                (&line[..index], value.strip_prefix(b" ").unwrap_or(value))
            }
            None => (line, &b""[..]),
        };

        match field {
            b"event" => {
                self.event = Some(String::from_utf8_lossy(value).into_owned());
                self.has_fields = true;
            }
            b"data" => {
                self.data.push(String::from_utf8_lossy(value).into_owned());
                self.has_fields = true;
            }
            _ => {
                // `id` and `retry` fields are kept in `raw` for passthrough.
                self.has_fields = true;
            }
        }
    }
}
