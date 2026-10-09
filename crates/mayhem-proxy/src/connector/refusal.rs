//! Audited refusal contracts, not substring guesses or automatic retries.
//! See REFUSALS.md for the pinned upstream control flow and limits.
use super::{
    config::{ErrorProfile, Operation},
    failure::{Execution, Failure, Scope},
};
use serde::{
    de::{IgnoredAny, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;

#[derive(Deserialize)]
struct Shape {
    n: Option<u64>,
    best_of: Option<u64>,
    use_beam_search: Option<bool>,
    messages: Option<Messages>,
    prompt: Option<Prompt>,
}
struct Messages;
impl<'de> Deserialize<'de> for Messages {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Messages;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a message array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Messages, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Messages)
            }
        }
        d.deserialize_seq(V)
    }
}
struct Prompt(bool);
impl<'de> Deserialize<'de> for Prompt {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Prompt;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("one string or token array")
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Prompt, E> {
                Ok(Prompt(true))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Prompt, A::Error> {
                let mut any = false;
                while seq.next_element::<u64>()?.is_some() {
                    any = true;
                }
                Ok(Prompt(any))
            }
        }
        d.deserialize_any(V)
    }
}

/// A completion batch may have started one prompt before refusing another. Only
/// a single engine invocation can use this contract. Responses can run multiple
/// engine steps; decisions have no vLLM admission contract.
pub(super) fn eligible(profile: ErrorProfile, operation: Operation, body: Option<&[u8]>) -> bool {
    if !matches!(profile, ErrorProfile::VllmAdmissionV1)
        || !matches!(
            operation,
            Operation::ChatCompletions | Operation::Completions
        )
    {
        return false;
    }
    // Inspect once per POST, without cloning prompt strings, tools, message
    // arrays or token vectors. Duplicate controls fail deserialization.
    let Some(shape) = body.and_then(|b| serde_json::from_slice::<Shape>(b).ok()) else {
        return false;
    };
    if shape.n.is_some_and(|v| v != 1)
        || shape.best_of.is_some_and(|v| v != 1)
        || shape.use_beam_search == Some(true)
    {
        return false;
    }
    match operation {
        Operation::ChatCompletions => shape.messages.is_some(),
        Operation::Completions => shape.prompt.is_some_and(|p| p.0),
        _ => false,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    error: Error,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Error {
    message: String,
    #[serde(rename = "type")]
    kind: String,
    param: Option<Value>,
    code: u16,
}

/// Only a complete bounded JSON HTTP error body from the broker can enter here.
/// Derived Deserialize rejects duplicate fields; extra result/job fields fail.
pub(super) fn recognize(failure: &mut Failure, bytes: &[u8]) {
    if failure.upstream_status != Some(503) {
        return;
    }
    let Ok(envelope) = serde_json::from_slice::<Envelope>(bytes) else {
        return;
    };
    let error = envelope.error;
    if error.code != 503 || error.kind != "Service Unavailable" || error.param.is_some() {
        return;
    }
    let code = match error.message.as_str() {
        "The engine is currently busy and cannot accept new requests. Please try again later or on a different instance." => "vllm_queue_overflow",
        "The engine has reached its prefill token backlog limit. Please try again later or on a different instance." => "vllm_prefill_backlog",
        _ => return,
    };
    failure.execution = Execution::Rejected;
    // vLLM's queue/backlog counters belong to this engine, including its aliases.
    failure.scope = Scope::Connection;
    failure.upstream_code = Some(code);
    failure.parameter = None;
}
