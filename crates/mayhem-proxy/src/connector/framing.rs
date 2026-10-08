//! Incremental framing, not inference success validation. Consumers must verify
//! their own finish/error events, tool boundaries and usage evidence. No
//! reconnect, job execution or money logic belongs in this decoder.

use super::{
    failure::{Code, Execution, Failure, Scope, Stage},
    http::WireFormat,
};

pub enum Frame {
    Sse {
        event: String,
        data: String,
        id: Option<String>,
    },
    Ndjson(serde_json::Value),
}

pub struct Decoder {
    format: WireFormat,
    limit: usize,
    line: Vec<u8>,
    data: String,
    event: String,
    id: Option<String>,
    has_data: bool,
    first_line: bool,
    skip_lf: bool,
    terminal: bool,
    failed: bool,
}

impl Decoder {
    pub fn new(format: WireFormat, max_event_bytes: usize) -> Result<Self, Failure> {
        if !matches!(format, WireFormat::Sse | WireFormat::Ndjson)
            || !(1..=256 * 1024 * 1024).contains(&max_event_bytes)
        {
            return Err(Self::error(Code::UpstreamProtocol));
        }
        Ok(Self {
            format,
            limit: max_event_bytes,
            line: Vec::new(),
            data: String::new(),
            event: String::new(),
            id: None,
            has_data: false,
            first_line: true,
            skip_lf: false,
            terminal: false,
            failed: false,
        })
    }

    fn error(code: Code) -> Failure {
        Failure::new(code, Scope::Model, Stage::ResponseBody, Execution::Unknown)
    }

    /// Emit through a callback, never an unbounded result vector. Errors latch;
    /// later input cannot turn failure into successful EOF.
    pub fn push(
        &mut self,
        chunk: &[u8],
        mut emit: impl FnMut(Frame) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        if self.failed || self.terminal {
            return Err(Self::error(Code::UpstreamProtocol));
        }
        let result = self.push_inner(chunk, &mut emit);
        if result.is_err() {
            self.failed = true;
            self.clear();
        }
        result
    }

    fn push_inner(
        &mut self,
        chunk: &[u8],
        emit: &mut impl FnMut(Frame) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        for byte in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            if *byte == b'\n' || *byte == b'\r' {
                self.consume_line(emit)?;
                self.skip_lf = *byte == b'\r';
            } else {
                if self.line.len() >= self.limit {
                    return Err(Self::error(Code::ResponseTooLarge));
                }
                self.line.push(*byte);
            }
        }
        Ok(())
    }

    fn consume_line(
        &mut self,
        emit: &mut impl FnMut(Frame) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        let line = std::mem::take(&mut self.line);
        let mut text =
            std::str::from_utf8(&line).map_err(|_| Self::error(Code::UpstreamProtocol))?;
        if self.first_line {
            text = text.strip_prefix('\u{feff}').unwrap_or(text);
            self.first_line = false;
        }
        if self.format == WireFormat::Ndjson {
            if !text.trim().is_empty() {
                let value =
                    serde_json::from_str(text).map_err(|_| Self::error(Code::UpstreamProtocol))?;
                emit(Frame::Ndjson(value))?;
            }
        } else if text.is_empty() {
            if self.has_data {
                self.data.pop();
                emit(Frame::Sse {
                    event: if self.event.is_empty() {
                        "message".into()
                    } else {
                        std::mem::take(&mut self.event)
                    },
                    data: std::mem::take(&mut self.data),
                    id: self.id.clone(),
                })?;
            }
            self.has_data = false;
            self.event.clear();
        } else if !text.starts_with(':') {
            let (field, value) = text.split_once(':').unwrap_or((text, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => {
                    if self.data.len().saturating_add(value.len() + 1) > self.limit {
                        return Err(Self::error(Code::ResponseTooLarge));
                    }
                    self.data.push_str(value);
                    self.data.push('\n');
                    self.has_data = true;
                }
                "event" => self.event = value.to_owned(),
                "id" if !value.contains('\0') => self.id = Some(value.to_owned()),
                // SSE retry instructions never trigger an implicit reconnect.
                _ => {}
            }
        }
        if self
            .data
            .len()
            .saturating_add(self.event.len())
            .saturating_add(self.id.as_ref().map_or(0, String::len))
            > self.limit
        {
            return Err(Self::error(Code::ResponseTooLarge));
        }
        // Reuse allocation without retaining a history of previous lines/events.
        self.line = line;
        self.line.clear();
        Ok(())
    }

    fn clear(&mut self) {
        self.line.clear();
        self.data.clear();
        self.event.clear();
        self.id = None;
        self.has_data = false;
    }

    pub fn finish(
        &mut self,
        mut emit: impl FnMut(Frame) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        if self.failed {
            return Err(Self::error(Code::UpstreamProtocol));
        }
        if self.terminal {
            return Ok(());
        }
        // Complete NDJSON can omit its final newline. SSE requires an empty-line
        // event boundary; partial events must never be dispatched as success.
        let result = if self.format == WireFormat::Ndjson && !self.line.is_empty() {
            self.consume_line(&mut emit)
        } else if self.has_data
            || std::str::from_utf8(&self.line)
                .map_or(true, |line| line == "data" || line.starts_with("data:"))
        {
            Err(Self::error(Code::UpstreamProtocol))
        } else {
            Ok(())
        };
        self.terminal = true;
        if result.is_err() {
            self.failed = true;
        }
        self.clear();
        result
    }
}
