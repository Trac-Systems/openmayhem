//! Called only by the contained stdio executable. Never parse tokenizer models
//! or execute their normalizers/regular expressions in the provider process.
use super::{Field, Limits};
use crate::attempts::Digest;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use tokenizers::Tokenizer;

pub const ABI: u32 = 1;
pub const CONTROL: usize = 4096;
pub const MAX_ARTIFACT: usize = 64 * 1024 * 1024;
pub const MAX_OUTPUT: usize = 4 * 1024 * 1024;
pub const HEAP_BYTES: usize = 512 * 1024 * 1024;
pub const WALL_SECONDS: u64 = 10;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Init {
    pub abi: u32,
    pub release: String,
    pub nonce: Digest,
    pub digest: Digest,
    pub limits: Limits,
    pub fields: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reply {
    pub abi: u32,
    pub release: String,
    pub nonce: Digest,
    pub digest: Digest,
    pub tokens: u64,
}
pub(crate) fn load(bytes: &[u8], digest: &Digest, limits: Limits) -> Result<Tokenizer, ()> {
    super::check(bytes, digest, limits).map_err(|_| ())?;
    let spec: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if !spec["padding"].is_null()
        || !spec["truncation"].is_null()
        || (!spec["model"]["dropout"].is_null() && spec["model"]["dropout"].as_f64() != Some(0.0))
    {
        return Err(());
    }
    let mut tokenizer = Tokenizer::from_bytes(bytes).map_err(|_| ())?;
    let mut model = tokenizer.get_model().clone();
    match &mut model {
        tokenizers::models::ModelWrapper::BPE(m) => m.resize_cache(0),
        tokenizers::models::ModelWrapper::Unigram(m) => m.resize_cache(0),
        _ => (),
    }
    tokenizer.with_model(model);
    Ok(tokenizer)
}
pub(crate) fn count(tokenizer: &Tokenizer, field: &Field) -> Result<u64, ()> {
    if !field.text.is_char_boundary(field.first_bytes) {
        return Err(());
    }
    let encoded = tokenizer
        .encode(field.text.as_str(), false)
        .map_err(|_| ())?;
    let mut count = 0u64;
    for &(start, end) in encoded.get_offsets() {
        if start > end || end > field.text.len() {
            return Err(());
        }
        if start >= field.first_bytes && end > start {
            count = count.checked_add(1).ok_or(())?;
        }
    }
    Ok(count)
}
pub(crate) fn read(r: &mut impl Read, max: usize) -> Result<Vec<u8>, ()> {
    let mut header = [0; 4];
    r.read_exact(&mut header).map_err(|_| ())?;
    let n = u32::from_le_bytes(header) as usize;
    if n > max {
        return Err(());
    }
    let mut bytes = vec![0; n];
    r.read_exact(&mut bytes).map_err(|_| ())?;
    Ok(bytes)
}
fn write(w: &mut impl Write, bytes: &[u8]) -> Result<(), ()> {
    if bytes.len() > CONTROL {
        return Err(());
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(|_| ())?;
    w.write_all(bytes).map_err(|_| ())?;
    w.flush().map_err(|_| ())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub nonce: Digest,
    pub fields: usize,
}
fn fields(
    input: &mut impl Read,
    tokenizer: &Tokenizer,
    limits: Limits,
    fields: usize,
) -> Result<u64, ()> {
    if fields > limits.channels {
        return Err(());
    }
    let mut remaining = limits.output_bytes;
    let mut tokens = 0u64;
    for _ in 0..fields {
        let mut first = [0; 4];
        input.read_exact(&mut first).map_err(|_| ())?;
        let bytes = read(input, remaining)?;
        remaining -= bytes.len();
        let field = Field {
            text: String::from_utf8(bytes).map_err(|_| ())?,
            first_bytes: u32::from_le_bytes(first) as usize,
        };
        tokens = tokens.checked_add(count(tokenizer, &field)?).ok_or(())?;
    }
    Ok(tokens)
}
fn reply(output: &mut impl Write, init: &Init, nonce: Digest, tokens: u64) -> Result<(), ()> {
    write(
        output,
        &serde_json::to_vec(&Reply {
            abi: init.abi,
            release: crate::worker::RELEASE.into(),
            nonce,
            digest: init.digest.clone(),
            tokens,
        })
        .map_err(|_| ())?,
    )
}
/// No paths, URLs, credentials, configuration discovery or remote code. The
/// caller must first install OS containment and fixed local resource ceilings.
pub fn run(mut input: impl Read, mut output: impl Write, persistent: bool) -> Result<(), ()> {
    tokenizers::utils::parallelism::set_parallelism(false);
    let init: Init = serde_json::from_slice(&read(&mut input, CONTROL)?).map_err(|_| ())?;
    if init.abi != if persistent { 2 } else { ABI }
        || init.release != crate::worker::RELEASE
        || init.fields > init.limits.channels
        || (persistent && init.fields != 0)
    {
        return Err(());
    }
    super::check_limits(init.limits).map_err(|_| ())?;
    let data = read(&mut input, init.limits.artifact_bytes)?;
    let tokenizer = load(&data, &init.digest, init.limits)?;
    drop(data);
    if !persistent {
        let tokens = fields(&mut input, &tokenizer, init.limits, init.fields)?;
        let mut trailing = [0];
        if input.read(&mut trailing).map_err(|_| ())? != 0 {
            return Err(());
        }
        return reply(&mut output, &init, init.nonce.clone(), tokens);
    }
    reply(&mut output, &init, init.nonce.clone(), 0)?;
    loop {
        let mut header = [0; 4];
        if input.read(&mut header[..1]).map_err(|_| ())? == 0 {
            return Ok(());
        }
        input.read_exact(&mut header[1..]).map_err(|_| ())?;
        let n = u32::from_le_bytes(header) as usize;
        if n > CONTROL {
            return Err(());
        }
        let mut bytes = vec![0; n];
        input.read_exact(&mut bytes).map_err(|_| ())?;
        let job: Job = serde_json::from_slice(&bytes).map_err(|_| ())?;
        let tokens = fields(&mut input, &tokenizer, init.limits, job.fields)?;
        reply(&mut output, &init, job.nonce, tokens)?;
    }
}

/// Fixed trusted startup policy, installed before the OS syscall filter. Never
/// apply this lifetime CPU limit to the streaming inference decoder mode.
pub fn resource_limits(persistent: bool) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use rustix::process::{setrlimit, Resource, Rlimit};
        if !persistent {
            setrlimit(
                Resource::Cpu,
                Rlimit {
                    current: Some(5),
                    maximum: Some(5),
                },
            )
            .map_err(|_| ())?;
        }
        #[cfg(target_os = "linux")]
        setrlimit(
            Resource::As,
            Rlimit {
                current: Some(768 * 1024 * 1024),
                maximum: Some(768 * 1024 * 1024),
            },
        )
        .map_err(|_| ())?;
        Ok(())
    }
    #[cfg(windows)]
    {
        // The trusted launcher establishes the immutable Job ceiling before
        // resume. Independently verify its exact tokenizer limit and all LPAC
        // controls before reading any artifact/IPC. One-shot v1 remains refused:
        // only persistent v2 has the existing parent per-measurement deadline.
        if !persistent {
            return Err(());
        }
        mayhem_windows_sandbox::verify_tokenizer_process().map_err(|_| ())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(())
    }
}
