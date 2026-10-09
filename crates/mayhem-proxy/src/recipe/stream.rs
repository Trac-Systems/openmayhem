//! Bounded event normalization only; endpoint assembly and terminal/schema
//! verification remain the existing common-protocol implementation.
use super::{check, transform, Error, ErrorKind, Projection, Result};
use crate::{
    connector::{
        failure::{Code, Execution, Failure, Scope, Stage},
        framing::Frame,
        http::WireFormat,
    },
    worker::Decoded,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Sse,
    Ndjson,
}
impl Format {
    pub fn wire(self) -> WireFormat {
        match self {
            Self::Sse => WireFormat::Sse,
            Self::Ndjson => WireFormat::Ndjson,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub format: Format,
    pub event_path: Vec<String>,
    pub events: BTreeMap<String, Event>,
    pub max_events: u32,
    pub fixtures: Vec<Sample>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Data {
        event: String,
        terminal: bool,
        value: Projection,
    },
    Finish,
    Error {
        error: ErrorKind,
    },
    Heartbeat,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub input: Vec<Value>,
    pub output: Vec<Value>,
}
#[derive(Default)]
pub(crate) struct State {
    events: u32,
    bytes: usize,
    finished: bool,
}
fn bad() -> Failure {
    Failure::new(
        Code::UpstreamProtocol,
        Scope::Model,
        Stage::ResponseBody,
        Execution::Unknown,
    )
}
impl Stream {
    pub(super) fn validate(
        &self,
        max: usize,
        endpoint: mayhem_proto::proxy::ProxyEndpoint,
    ) -> Result<()> {
        transform::path(&self.event_path)?;
        check(
            !self.event_path.is_empty()
                && (1..=64).contains(&self.events.len())
                && (1..=65_536).contains(&self.max_events)
                && (1..=4).contains(&self.fixtures.len()),
        )?;
        for (key, event) in &self.events {
            check(transform::key(key))?;
            match event {
                Event::Data {
                    event,
                    terminal,
                    value,
                } => {
                    check(transform::key(event))?;
                    check(
                        !terminal
                            || (endpoint == mayhem_proto::proxy::ProxyEndpoint::Responses
                                && matches!(
                                    event.as_str(),
                                    "response.completed" | "response.incomplete"
                                )),
                    )?;
                    value.validate()?;
                }
                Event::Finish => check(endpoint != mayhem_proto::proxy::ProxyEndpoint::Responses)?,
                _ => (),
            }
        }
        for fixture in &self.fixtures {
            check((1..=32).contains(&fixture.input.len()))?;
            let mut state = State::default();
            let mut output = Vec::new();
            for value in &fixture.input {
                if let Some(frame) = self.map(value, max, &mut state).map_err(|_| Error)? {
                    output.push(serde_json::to_value(frame).map_err(|_| Error)?);
                }
            }
            check(output == fixture.output && state.finished)?;
        }
        Ok(())
    }
    pub(crate) fn frame(
        &self,
        frame: Frame,
        max: usize,
        state: &mut State,
    ) -> std::result::Result<Option<Decoded>, Failure> {
        let input = match (frame, self.format) {
            (Frame::Sse { event, data, id }, Format::Sse) => {
                json!({"event":event,"data":serde_json::from_str::<Value>(&data).map_err(|_|bad())?,"id":id})
            }
            (Frame::Ndjson(value), Format::Ndjson) => json!({"event":null,"data":value,"id":null}),
            _ => return Err(bad()),
        };
        self.map(&input, max, state)
    }
    fn map(
        &self,
        input: &Value,
        max: usize,
        state: &mut State,
    ) -> std::result::Result<Option<Decoded>, Failure> {
        if state.finished || state.events >= self.max_events {
            return Err(bad());
        }
        state.events += 1;
        transform::bounded(input, max).map_err(|_| bad())?;
        let key = transform::read(input, &self.event_path)
            .and_then(Value::as_str)
            .ok_or_else(bad)?;
        let frame = match self.events.get(key).ok_or_else(bad)? {
            Event::Heartbeat => return Ok(None),
            Event::Error { error } => return Err(error.failure()),
            Event::Finish => {
                state.finished = true;
                Decoded::Sse {
                    event: "message".into(),
                    data: "[DONE]".into(),
                    id: None,
                }
            }
            Event::Data {
                event,
                terminal,
                value,
            } => {
                let output = value.apply(input, max).map_err(|_| bad())?;
                state.finished = *terminal;
                Decoded::Sse {
                    event: event.clone(),
                    data: serde_json::to_string(&output).map_err(|_| bad())?,
                    id: None,
                }
            }
        };
        let bytes = serde_json::to_vec(&frame).map_err(|_| bad())?.len();
        state.bytes = state.bytes.checked_add(bytes).ok_or_else(bad)?;
        if state.bytes > max {
            return Err(bad());
        }
        Ok(Some(frame))
    }
    pub(crate) fn finish(&self, state: &State) -> std::result::Result<(), Failure> {
        if state.finished {
            Ok(())
        } else {
            Err(bad())
        }
    }
}
