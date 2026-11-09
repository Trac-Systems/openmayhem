//! Constant-space observation of the synchronous OpenAI SSE terminal marker.
//! This never parses output JSON, forwards content, accepts a result or meters it.
//! Full response validation remains in the isolated decoder. Keeping framing
//! position here lets a failed decoder retain execution-end evidence safely.

pub(super) struct SseTerminal {
    line: [u8; 64],
    len: usize,
    overflow: bool,
    skip_lf: bool,
    data_seen: bool,
    done_data: bool,
    message_event: bool,
}
impl Default for SseTerminal {
    fn default() -> Self {
        Self {
            line: [0; 64],
            len: 0,
            overflow: false,
            skip_lf: false,
            data_seen: false,
            done_data: false,
            message_event: true,
        }
    }
}
impl SseTerminal {
    pub(super) fn push(&mut self, bytes: &[u8]) -> bool {
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\n' || byte == b'\r' {
                let done = self.end_line();
                self.skip_lf = byte == b'\r';
                if done {
                    return true;
                }
            } else if self.len < self.line.len() {
                self.line[self.len] = byte;
                self.len += 1;
            } else {
                self.overflow = true;
            }
        }
        false
    }
    fn end_line(&mut self) -> bool {
        let line = &self.line[..self.len];
        let done = if line.is_empty() {
            let done = self.data_seen && self.done_data && self.message_event;
            self.data_seen = false;
            self.done_data = false;
            self.message_event = true;
            done
        } else {
            let (field, value) = line
                .iter()
                .position(|&b| b == b':')
                .map(|at| (&line[..at], &line[at + 1..]))
                .unwrap_or((line, &[]));
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if field == b"data" {
                // A second data line is not the exact terminal event, including
                // an empty extra line. Never search for [DONE] inside payloads.
                self.done_data = !self.data_seen && !self.overflow && value == b"[DONE]";
                self.data_seen = true;
            } else if field == b"event" {
                self.message_event = !self.overflow && (value.is_empty() || value == b"message");
            }
            false
        };
        self.len = 0;
        self.overflow = false;
        done
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_marker_survives_all_chunk_boundaries_and_line_endings() {
        for separator in ["\n", "\r\n", "\r"] {
            let wire = format!("data: {{\"text\":\"{}\"}}{separator}{separator}: comment{separator}event: message{separator}data: [DONE]{separator}{separator}", "x".repeat(500));
            for size in 1..=wire.len() {
                let mut observer = SseTerminal::default();
                assert!(wire.as_bytes().chunks(size).any(|b| observer.push(b)));
            }
        }
    }
    #[test]
    fn embedded_partial_multiline_and_wrong_event_markers_are_not_terminal() {
        for wire in [
            "data: {\"text\":\"[DONE]\"}\n\n",
            "data: [DONE]\n",
            "data: text\ndata: [DONE]\n\n",
            "data: [DONE]\ndata:\n\n",
            "event: other\ndata: [DONE]\n\n",
            ":data: [DONE]\n\n",
            "xdata: [DONE]\n\n",
            "data: [DONE]suffix\n\n",
            "data: \0[DONE]\n\n",
        ] {
            for size in 1..=wire.len() {
                let mut observer = SseTerminal::default();
                assert!(
                    !wire.as_bytes().chunks(size).any(|b| observer.push(b)),
                    "{wire:?}"
                );
            }
        }
        let mut observer = SseTerminal::default();
        assert!(!observer.push(format!("data: {}[DONE]\n\n", "x".repeat(100_000)).as_bytes()));
        assert!(observer.push(b"data: [DONE]\n\n"));
    }
}
