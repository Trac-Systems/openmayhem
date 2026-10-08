//! Bundled decoder process, outside the financial kernel. Networking/credentials
//! stay in the parent. IPC carries one bound attempt's model bytes and parsed
//! frames, never receipt signing requests, prices, filesystem paths or URLs to fetch.
//! Framing/JSON success alone is not endpoint validity. Optional bound schema
//! verification adds semantic checks but never permission to settle a receipt.

pub mod host;
mod wire;

use crate::{
    attempts::{Binding, Digest, FailureSnapshot, Record},
    connector::{
        config::ErrorProfile,
        failure::{self, Code, Execution, Failure, Scope, Stage},
        framing::{Decoder, Frame},
        http::WireFormat,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{Read, Write},
};
use wire::*;

pub const ABI: u32 = 2;
pub const RELEASE: &str = env!("CARGO_PKG_VERSION");
pub const CHUNK_BYTES: usize = 64 * 1024;
const CONTROL_BYTES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("proxy decoder configuration is invalid")]
    Configuration,
    #[error("proxy decoder has no available local process capacity")]
    Capacity,
    #[error("proxy decoder could not start")]
    Start,
    #[error("proxy decoder version or attempt binding does not match")]
    Identity,
    #[error("proxy decoder protocol failed")]
    Protocol,
    #[error("proxy decoder stopped")]
    Stopped,
    #[error("proxy decoding was cancelled; upstream execution may continue")]
    Cancelled,
    #[error("proxy decoder exceeded its local processing deadline")]
    ProcessingTimeout,
    #[error("proxy decoder rejected upstream response: {0}")]
    Upstream(Failure),
}
pub type Result<T> = std::result::Result<T, Error>;

fn protocol() -> Failure {
    Failure::new(
        Code::UpstreamProtocol,
        Scope::Model,
        Stage::ResponseBody,
        Execution::Unknown,
    )
}
fn too_large() -> Failure {
    Failure::new(
        Code::ResponseTooLarge,
        Scope::Model,
        Stage::ResponseBody,
        Execution::Unknown,
    )
}

/// Session identity contains hashes only, not a wallet, secret or raw prompt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub invocation: Digest,
    pub attempt: u64,
    pub binding_hash: Digest,
}
impl Session {
    pub fn from_record(record: &Record) -> Result<Self> {
        let hash = binding_hash(&record.binding)?;
        if record.attempt == 0 {
            return Err(Error::Configuration);
        }
        Ok(Self {
            invocation: record.invocation.clone(),
            attempt: record.attempt,
            binding_hash: hash,
        })
    }
}
fn binding_hash(binding: &Binding) -> Result<Digest> {
    let bytes = serde_json::to_vec(binding).map_err(|_| Error::Configuration)?;
    let mut h = blake3::Hasher::new_derive_key("mayhem/proxy/decoder-binding/v1");
    h.update(&bytes);
    Digest::new(h.finalize().to_hex().as_str()).map_err(|_| Error::Configuration)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodeLimits {
    pub max_total_bytes: usize,
    pub max_event_bytes: usize,
}
impl DecodeLimits {
    fn validate(&self) -> Result<()> {
        if !(1..=256 * 1024 * 1024).contains(&self.max_total_bytes)
            || !(1..=self.max_total_bytes).contains(&self.max_event_bytes)
        {
            return Err(Error::Configuration);
        }
        Ok(())
    }
    fn ipc_bytes(&self) -> usize {
        self.max_event_bytes * 8 + CONTROL_BYTES
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Init {
    pub abi: u32,
    pub release: String,
    pub session: Session,
    pub format: WireFormat,
    pub error_profile: ErrorProfile,
    pub limits: DecodeLimits,
    pub semantic_policy: Option<Digest>,
}
impl Init {
    pub fn new(
        record: &Record,
        format: WireFormat,
        error_profile: ErrorProfile,
        limits: DecodeLimits,
    ) -> Result<Self> {
        let init = Self {
            abi: ABI,
            release: RELEASE.into(),
            session: Session::from_record(record)?,
            format,
            error_profile,
            limits,
            semantic_policy: None,
        };
        init.validate()?;
        Ok(init)
    }
    pub fn with_semantics(mut self, policy: &crate::semantics::Policy) -> Result<Self> {
        self.semantic_policy = Some(policy.digest().map_err(Error::Upstream)?);
        self.validate()?;
        Ok(self)
    }
    fn validate(&self) -> Result<()> {
        self.limits.validate()?;
        if self.semantic_policy.is_some()
            && !matches!(self.format, WireFormat::Json | WireFormat::Sse)
        {
            return Err(Error::Configuration);
        }
        if self.abi != ABI || self.release != RELEASE || self.session.attempt == 0 {
            return Err(Error::Identity);
        }
        if self.format == WireFormat::Unknown {
            return Err(Error::Configuration);
        }
        Ok(())
    }
}

/// Untrusted decoded model content. A parent endpoint adapter must validate role,
/// tool/choice associations, finish conditions and independently checkable usage.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decoded {
    Json {
        value: serde_json::Value,
    },
    Ndjson {
        value: serde_json::Value,
    },
    Sse {
        event: String,
        data: String,
        id: Option<String>,
    },
}
impl fmt::Debug for Decoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Json { .. } => "Decoded::Json([redacted])",
            Self::Ndjson { .. } => "Decoded::Ndjson([redacted])",
            Self::Sse { .. } => "Decoded::Sse([redacted])",
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    abi: u32,
    release: String,
    session: Session,
}

fn json<T: Serialize>(value: &T, max: usize) -> Result<Vec<u8>> {
    struct Limited {
        bytes: Vec<u8>,
        max: usize,
    }
    impl Write for Limited {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if b.len() > self.max.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("IPC size limit"));
            }
            self.bytes.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Limited {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::Protocol)?;
    Ok(writer.bytes)
}

fn emit(
    w: &mut impl Write,
    event: Decoded,
    init: &Init,
    verifier: Option<&crate::semantics::Verifier>,
) -> std::result::Result<(), Failure> {
    if matches!(init.error_profile, ErrorProfile::OpenAi) {
        let body = match &event {
            Decoded::Sse { data, .. } => data.as_bytes().to_vec(),
            Decoded::Json { value } | Decoded::Ndjson { value } => {
                serde_json::to_vec(value).map_err(|_| protocol())?
            }
        };
        if let Some(failure) = failure::openai_stream_error(&body) {
            return Err(failure);
        }
        // Diagnostic extraction is intentionally small, but an oversized error
        // envelope must not escape that bound by becoming successful model data.
        if body.len() > 16 * 1024 {
            let value = serde_json::from_slice::<serde_json::Value>(&body).ok();
            if value.as_ref().is_some_and(|v| {
                v.get("error").is_some_and(serde_json::Value::is_object)
                    || v.get("type").and_then(serde_json::Value::as_str) == Some("error")
            }) {
                return Err(protocol());
            }
        }
    }
    if let Some(verifier) = verifier {
        match &event {
            Decoded::Json { value } => verifier.verify(value)?,
            _ => return Err(protocol()),
        }
    }
    let bytes = json(&event, init.limits.ipc_bytes()).map_err(|_| too_large())?;
    write_packet(w, EVENT, &bytes).map_err(|_| protocol())
}
fn emit_frame(w: &mut impl Write, frame: Frame, init: &Init) -> std::result::Result<(), Failure> {
    let event = match frame {
        Frame::Sse { event, data, id } => Decoded::Sse { event, data, id },
        Frame::Ndjson(value) => Decoded::Ndjson { value },
    };
    emit(w, event, init, None)
}

/// One attempt, then exit. No shell, network, model execution or filesystem API.
/// stdin EOF/error without a valid Finish is a failure, never a successful finish.
pub fn run(mut input: impl Read, mut output: impl Write) -> Result<()> {
    let hello = read_packet(&mut input, CONTROL_BYTES)?.ok_or(Error::Protocol)?;
    if hello.kind != HELLO {
        return Err(Error::Protocol);
    }
    let init: Init = serde_json::from_slice(&hello.bytes).map_err(|_| Error::Protocol)?;
    init.validate()?;
    let ready = Ready {
        abi: ABI,
        release: RELEASE.into(),
        session: init.session.clone(),
    };
    write_packet(&mut output, READY, &json(&ready, CONTROL_BYTES)?)?;
    let mut decoder = if init.format == WireFormat::Json {
        None
    } else {
        Some(Decoder::new(init.format, init.limits.max_event_bytes).map_err(Error::Upstream)?)
    };
    let mut bytes = Vec::new();
    let mut policy_bytes = Vec::new();
    let mut verifier = None;
    let mut frames_ended = false;
    let mut received = 0usize;
    let mut sequence = 0u64;
    loop {
        let packet = read_packet(&mut input, CHUNK_BYTES)?.ok_or(Error::Protocol)?;
        sequence = sequence.checked_add(1).ok_or(Error::Protocol)?;
        let result = match packet.kind {
            POLICY_CHUNK
                if init.semantic_policy.is_some()
                    && verifier.is_none()
                    && received == 0
                    && !packet.bytes.is_empty() =>
            {
                if packet.bytes.len()
                    > crate::semantics::MAX_POLICY_BYTES.saturating_sub(policy_bytes.len())
                {
                    Err(crate::semantics::bad_schema("tools"))
                } else {
                    policy_bytes.extend_from_slice(&packet.bytes);
                    Ok(())
                }
            }
            POLICY_END
                if init.semantic_policy.is_some()
                    && verifier.is_none()
                    && received == 0
                    && packet.bytes.is_empty() =>
            {
                let compile = || -> std::result::Result<crate::semantics::Verifier, Failure> {
                    let policy: crate::semantics::Policy = serde_json::from_slice(&policy_bytes)
                        .map_err(|_| crate::semantics::bad_schema("tools"))?;
                    if Some(policy.digest()?) != init.semantic_policy {
                        return Err(crate::semantics::bad_schema("tools"));
                    }
                    crate::semantics::Verifier::new(policy)
                };
                match compile() {
                    Ok(v) => {
                        verifier = Some(v);
                        policy_bytes = Vec::new();
                        Ok(())
                    }
                    Err(f) => Err(f),
                }
            }
            CHUNK
                if !frames_ended
                    && !packet.bytes.is_empty()
                    && (init.semantic_policy.is_none() || verifier.is_some()) =>
            {
                if packet.bytes.len() > init.limits.max_total_bytes.saturating_sub(received) {
                    Err(too_large())
                } else {
                    received += packet.bytes.len();
                    if let Some(decoder) = &mut decoder {
                        decoder.push(&packet.bytes, |event| emit_frame(&mut output, event, &init))
                    } else if packet.bytes.len()
                        > init.limits.max_event_bytes.saturating_sub(bytes.len())
                    {
                        Err(too_large())
                    } else {
                        bytes.extend_from_slice(&packet.bytes);
                        Ok(())
                    }
                }
            }
            FINISH
                if packet.bytes.is_empty()
                    && !frames_ended
                    && !(verifier.is_some() && decoder.is_some())
                    && (init.semantic_policy.is_none() || verifier.is_some()) =>
            {
                if let Some(decoder) = &mut decoder {
                    decoder.finish(|event| emit_frame(&mut output, event, &init))
                } else {
                    serde_json::from_slice(&bytes)
                        .map_err(|_| protocol())
                        .and_then(|value| {
                            emit(
                                &mut output,
                                Decoded::Json { value },
                                &init,
                                verifier.as_ref(),
                            )
                        })
                }
            }
            FRAME_END
                if packet.bytes.is_empty()
                    && verifier.is_some()
                    && decoder.is_some()
                    && !frames_ended =>
            {
                frames_ended = true;
                decoder
                    .as_mut()
                    .ok_or(Error::Protocol)?
                    .finish(|event| emit_frame(&mut output, event, &init))
            }
            VERIFY_CHUNK if frames_ended && !packet.bytes.is_empty() => {
                if packet.bytes.len() > init.limits.max_event_bytes.saturating_sub(bytes.len()) {
                    Err(too_large())
                } else {
                    bytes.extend_from_slice(&packet.bytes);
                    Ok(())
                }
            }
            VERIFY_END if frames_ended && packet.bytes.is_empty() => serde_json::from_slice(&bytes)
                .map_err(|_| protocol())
                .and_then(|value| verifier.as_ref().ok_or_else(protocol)?.verify(&value)),
            _ => return Err(Error::Protocol),
        };
        if let Err(failure) = result {
            write_packet(
                &mut output,
                FAILURE,
                &json(&FailureSnapshot::from(&failure), CONTROL_BYTES)?,
            )?;
            return Err(Error::Upstream(failure));
        }
        if matches!(packet.kind, FINISH | VERIFY_END) {
            write_packet(&mut output, END, &sequence.to_le_bytes())?;
            return Ok(());
        }
        write_packet(&mut output, ACK, &sequence.to_le_bytes())?;
    }
}

/// Defense in depth for this trusted bundled decoder. This is process isolation,
/// not an OS filesystem/network sandbox for arbitrary compiled extensions.
pub fn disable_core_dumps() -> Result<()> {
    #[cfg(unix)]
    {
        rustix::process::setrlimit(
            rustix::process::Resource::Core,
            rustix::process::Rlimit {
                current: Some(0),
                maximum: Some(0),
            },
        )
        .map_err(|_| Error::Start)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn init(format: WireFormat) -> Init {
        Init {
            abi: ABI,
            release: RELEASE.into(),
            session: Session {
                invocation: Digest::new("1".repeat(64)).unwrap(),
                attempt: 1,
                binding_hash: Digest::new("2".repeat(64)).unwrap(),
            },
            format,
            error_profile: ErrorProfile::OpenAi,
            semantic_policy: None,
            limits: DecodeLimits {
                max_total_bytes: 128 * 1024,
                max_event_bytes: 32 * 1024,
            },
        }
    }
    fn input(init: &Init, packets: &[(u8, &[u8])]) -> Vec<u8> {
        let mut input = Vec::new();
        write_packet(&mut input, HELLO, &json(init, CONTROL_BYTES).unwrap()).unwrap();
        for (kind, bytes) in packets {
            write_packet(&mut input, *kind, bytes).unwrap();
        }
        input
    }
    fn kinds(output: &[u8]) -> Vec<u8> {
        let mut output = output;
        let mut result = Vec::new();
        while let Some(packet) = read_packet(&mut output, 1024 * 1024).unwrap() {
            result.push(packet.kind);
        }
        result
    }
    #[test]
    fn wrong_version_or_abi_has_no_ready_or_events() {
        for change_abi in [true, false] {
            let mut init = init(WireFormat::Json);
            if change_abi {
                init.abi += 1;
            } else {
                init.release = "0.0.0".into();
            }
            let mut output = Vec::new();
            assert!(matches!(
                run(input(&init, &[]).as_slice(), &mut output),
                Err(Error::Identity)
            ));
            assert!(output.is_empty());
        }
    }
    #[test]
    fn abrupt_input_eof_or_unknown_packet_has_no_finish() {
        let init = init(WireFormat::Json);
        for packets in [
            vec![(CHUNK, b"{}".as_slice())],
            vec![(99, b"{}".as_slice())],
            vec![(FINISH, b"unexpected".as_slice())],
        ] {
            let mut output = Vec::new();
            assert!(run(input(&init, &packets).as_slice(), &mut output).is_err());
            assert!(!kinds(&output).contains(&END));
        }
    }
    #[test]
    fn total_stream_limit_applies_even_when_every_event_is_small() {
        let mut init = init(WireFormat::Ndjson);
        init.limits.max_total_bytes = 10;
        init.limits.max_event_bytes = 4;
        let mut output = Vec::new();
        assert!(
            matches!(run(input(&init, &[(CHUNK,b"{}\n{}\n"),(CHUNK,b"{}\n{}\n"),(FINISH,b"")]).as_slice(), &mut output), Err(Error::Upstream(f)) if f.code == Code::ResponseTooLarge)
        );
        assert_eq!(kinds(&output), vec![READY, EVENT, EVENT, ACK, FAILURE]);
    }
    #[test]
    fn custom_profile_does_not_guess_error_semantics_from_a_data_field() {
        let mut init = init(WireFormat::Json);
        init.error_profile = ErrorProfile::HttpStatus;
        let mut output = Vec::new();
        run(
            input(
                &init,
                &[
                    (CHUNK, br#"{"error":{"local_classifier_field":true}}"#),
                    (FINISH, b""),
                ],
            )
            .as_slice(),
            &mut output,
        )
        .unwrap();
        assert_eq!(kinds(&output), vec![READY, ACK, EVENT, END]);
    }
    #[test]
    fn invalid_utf8_and_excessive_json_nesting_are_explicit_failures() {
        let nested = format!("{}0{}", "[".repeat(150), "]".repeat(150));
        for (format, body) in [
            (WireFormat::Json, nested.as_bytes()),
            (WireFormat::Ndjson, b"\xff\n".as_slice()),
        ] {
            let init = init(format);
            let mut output = Vec::new();
            assert!(
                matches!(run(input(&init,&[(CHUNK,body),(FINISH,b"")]).as_slice(), &mut output), Err(Error::Upstream(f)) if f.code == Code::UpstreamProtocol)
            );
            assert!(!kinds(&output).contains(&END));
        }
    }
}
