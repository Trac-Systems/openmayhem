use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wait_timeout::ChildExt;

use crate::ReputationEventKind;

pub const DEFAULT_CANARY_MATCH_MIN_BPS: u32 = 9_000;
pub const MIN_LAUNCH_CANARY_STABLE_PREFIX_TOKENS: usize = 64;
pub const CANARY_VERIFICATION_TOKEN_FINGERPRINT: &str = "token_fingerprint";
pub const CANARY_VERIFICATION_CONTEXT_NEEDLE: &str = "context_needle";
pub const CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH: &str = "seed_perceptual_hash";
pub const CANARY_VERIFICATION_EMBEDDING_COSINE: &str = "embedding_cosine";
pub const CANARY_VERIFICATION_TRANSCRIPT_MATCH: &str = "transcript_match";
pub const CANARY_VERIFICATION_AUDIO_FINGERPRINT: &str = "audio_fingerprint";
pub const CANARY_VERIFICATION_VIDEO_AV_FINGERPRINT: &str = "video_av_fingerprint";
pub const CANARY_VERIFICATION_ATTESTATION_OF_COMPUTE: &str = "attestation_of_compute";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanaryProbeSpec {
    pub model: String,
    pub canary_set: String,
    pub prompt_id: String,
    pub prompt: String,
    pub seed: i64,
    pub max_tokens: u32,
    #[serde(default)]
    pub sampling: CanarySamplingProfile,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CanarySamplingProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanaryTokenFingerprint {
    pub token_ids: Vec<i32>,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanaryProbeEvaluation {
    pub verification_method: String,
    pub canary_set: String,
    pub prompt_id: String,
    pub expected_fingerprint: String,
    pub observed_fingerprint: String,
    pub matched_positions: u32,
    pub total_positions: u32,
    pub match_bps: u32,
    pub pass: bool,
}

impl CanaryProbeSpec {
    pub fn openai_chat_body(&self) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": self.prompt,
                }
            ],
            "seed": self.seed,
            "max_tokens": self.max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        for (name, value) in [
            ("temperature", self.sampling.temperature.map(Value::from)),
            ("top_p", self.sampling.top_p.map(Value::from)),
            ("top_k", self.sampling.top_k.map(Value::from)),
            ("min_p", self.sampling.min_p.map(Value::from)),
            (
                "repeat_penalty",
                self.sampling.repeat_penalty.map(Value::from),
            ),
            (
                "frequency_penalty",
                self.sampling.frequency_penalty.map(Value::from),
            ),
            (
                "presence_penalty",
                self.sampling.presence_penalty.map(Value::from),
            ),
        ] {
            if let Some(value) = value {
                body[name] = value;
            }
        }
        body
    }
}

impl CanaryProbeEvaluation {
    pub fn reputation_event_kind(&self) -> ReputationEventKind {
        if self.pass {
            ReputationEventKind::ProbeOk
        } else {
            ReputationEventKind::ProbeFail
        }
    }

    pub fn provenance_violation(&self) -> bool {
        !self.pass
    }
}

pub fn token_fingerprint(token_ids: impl IntoIterator<Item = i32>) -> CanaryTokenFingerprint {
    let token_ids = token_ids.into_iter().collect::<Vec<_>>();
    let mut hasher = blake3::Hasher::new();
    for token_id in &token_ids {
        hasher.update(&token_id.to_be_bytes());
    }
    CanaryTokenFingerprint {
        token_ids,
        digest: hasher.finalize().to_hex().to_string(),
    }
}

pub fn supported_canary_verification_method(method: &str) -> bool {
    matches!(
        method,
        CANARY_VERIFICATION_TOKEN_FINGERPRINT
            | CANARY_VERIFICATION_CONTEXT_NEEDLE
            | CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH
            | CANARY_VERIFICATION_EMBEDDING_COSINE
            | CANARY_VERIFICATION_TRANSCRIPT_MATCH
            | CANARY_VERIFICATION_AUDIO_FINGERPRINT
            | CANARY_VERIFICATION_VIDEO_AV_FINGERPRINT
            | CANARY_VERIFICATION_ATTESTATION_OF_COMPUTE
    )
}

pub fn aggregate_canary_fingerprints<'a>(
    prompts: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for (prompt_id, fingerprint) in prompts {
        let prompt_id = prompt_id.as_bytes();
        hasher.update(&(prompt_id.len() as u32).to_be_bytes());
        hasher.update(prompt_id);
        hasher.update(fingerprint.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub fn evaluate_canary_probe(
    spec: &CanaryProbeSpec,
    expected: &[i32],
    observed: &[i32],
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let total_positions = expected.len() as u32;
    let matched_positions = expected
        .iter()
        .zip(observed.iter())
        .filter(|(expected, observed)| expected == observed)
        .count() as u32;
    let match_bps = if total_positions == 0 {
        0
    } else {
        ((u64::from(matched_positions) * 10_000) / u64::from(total_positions)) as u32
    };
    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint: token_fingerprint(expected.iter().copied()).digest,
        observed_fingerprint: token_fingerprint(observed.iter().copied()).digest,
        matched_positions,
        total_positions,
        match_bps,
        pass: match_bps >= min_match_bps,
    }
}

pub fn evaluate_catalog_canary_probe(
    spec: &CanaryProbeSpec,
    expected_fingerprint: &str,
    observed_fingerprint: &str,
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let match_bps = if expected_fingerprint.eq_ignore_ascii_case(observed_fingerprint) {
        10_000
    } else {
        0
    };
    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint: expected_fingerprint.to_owned(),
        observed_fingerprint: observed_fingerprint.to_owned(),
        matched_positions: u32::from(match_bps == 10_000),
        total_positions: 1,
        match_bps,
        pass: match_bps >= min_match_bps,
    }
}

pub fn evaluate_catalog_canary_token_prefix_probe(
    spec: &CanaryProbeSpec,
    expected_prefixes: &BTreeMap<String, Vec<i32>>,
    observed_tokens: &BTreeMap<String, Vec<i32>>,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut expected_fingerprints = Vec::with_capacity(expected_prefixes.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_prefixes.len());

    for (prompt_id, expected_prefix) in expected_prefixes {
        let observed = observed_tokens
            .get(prompt_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let observed_prefix = observed
            .iter()
            .copied()
            .take(expected_prefix.len())
            .collect::<Vec<_>>();
        total_positions = total_positions.saturating_add(expected_prefix.len() as u32);
        matched_positions = matched_positions.saturating_add(
            expected_prefix
                .iter()
                .zip(observed_prefix.iter())
                .filter(|(expected, observed)| expected == observed)
                .count() as u32,
        );
        let expected_fingerprint = token_fingerprint(expected_prefix.iter().copied()).digest;
        let observed_fingerprint = token_fingerprint(observed_prefix.iter().copied()).digest;
        expected_fingerprints.push((prompt_id.as_str(), expected_fingerprint));
        observed_fingerprints.push((prompt_id.as_str(), observed_fingerprint));
    }

    let match_bps = if total_positions == 0 {
        0
    } else {
        ((u64::from(matched_positions) * 10_000) / u64::from(total_positions)) as u32
    };
    let expected_fingerprint = aggregate_canary_fingerprints(
        expected_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );
    let observed_fingerprint = aggregate_canary_fingerprints(
        observed_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );

    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_TOKEN_FINGERPRINT.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint,
        observed_fingerprint,
        matched_positions,
        total_positions,
        match_bps,
        pass: total_positions > 0 && matched_positions == total_positions,
    }
}

pub fn evaluate_seed_perceptual_hash_probe(
    spec: &CanaryProbeSpec,
    expected_hash: &str,
    observed_hash: &str,
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let (matched_positions, total_positions, match_bps) =
        perceptual_hash_match_stats(expected_hash, observed_hash).unwrap_or((0, 1, 0));
    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint: expected_hash.to_ascii_lowercase(),
        observed_fingerprint: observed_hash.to_ascii_lowercase(),
        matched_positions,
        total_positions,
        match_bps,
        pass: match_bps >= min_match_bps,
    }
}

pub fn evaluate_catalog_canary_perceptual_hash_probe(
    spec: &CanaryProbeSpec,
    expected_hashes: &BTreeMap<String, String>,
    observed_hashes: &BTreeMap<String, String>,
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut expected_fingerprints = Vec::with_capacity(expected_hashes.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_hashes.len());

    for (prompt_id, expected_hash) in expected_hashes {
        let observed_hash = observed_hashes
            .get(prompt_id)
            .map(String::as_str)
            .unwrap_or_default();
        let (matched, total, _) = perceptual_hash_match_stats(expected_hash, observed_hash)
            .unwrap_or_else(|| (0, ((expected_hash.len() * 4).max(1)) as u32, 0));
        matched_positions = matched_positions.saturating_add(matched);
        total_positions = total_positions.saturating_add(total);
        expected_fingerprints.push((prompt_id.as_str(), expected_hash.to_ascii_lowercase()));
        observed_fingerprints.push((prompt_id.as_str(), observed_hash.to_ascii_lowercase()));
    }

    let match_bps = if total_positions == 0 {
        0
    } else {
        ((u64::from(matched_positions) * 10_000) / u64::from(total_positions)) as u32
    };
    let expected_fingerprint = aggregate_canary_fingerprints(
        expected_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );
    let observed_fingerprint = aggregate_canary_fingerprints(
        observed_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );

    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint,
        observed_fingerprint,
        matched_positions,
        total_positions,
        match_bps,
        pass: total_positions > 0 && match_bps >= min_match_bps,
    }
}

pub fn embedding_vector_fingerprint(values: &[f32]) -> String {
    let mut hasher = blake3::Hasher::new();
    for value in values {
        hasher.update(&value.to_le_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub fn embedding_cosine_similarity_bps(expected: &[f32], observed: &[f32]) -> Option<u32> {
    if expected.is_empty() || expected.len() != observed.len() {
        return None;
    }
    let (dot, expected_norm, observed_norm) = expected.iter().zip(observed).try_fold(
        (0.0_f64, 0.0_f64, 0.0_f64),
        |(dot, left_norm, right_norm), (left, right)| {
            if !left.is_finite() || !right.is_finite() {
                return None;
            }
            let left = f64::from(*left);
            let right = f64::from(*right);
            Some((
                dot + left * right,
                left_norm + left * left,
                right_norm + right * right,
            ))
        },
    )?;
    if expected_norm == 0.0 || observed_norm == 0.0 {
        return None;
    }
    let cosine = dot / (expected_norm.sqrt() * observed_norm.sqrt());
    if !cosine.is_finite() {
        return None;
    }
    Some((cosine.clamp(0.0, 1.0) * 10_000.0).round() as u32)
}

pub fn embedding_canary_matches(expected: &[f32], observed: &[f32], tolerance_bps: u32) -> bool {
    let min_bps = 10_000u32.saturating_sub(tolerance_bps);
    embedding_cosine_similarity_bps(expected, observed).is_some_and(|score| score >= min_bps)
}

pub fn evaluate_catalog_canary_embedding_cosine_probe(
    spec: &CanaryProbeSpec,
    expected_vectors: &BTreeMap<String, Vec<f32>>,
    observed_vectors: &BTreeMap<String, Vec<f32>>,
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut score_sum = 0_u64;
    let mut expected_fingerprints = Vec::with_capacity(expected_vectors.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_vectors.len());

    for (prompt_id, expected) in expected_vectors {
        let observed = observed_vectors
            .get(prompt_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let score = embedding_cosine_similarity_bps(expected, observed).unwrap_or(0);
        total_positions = total_positions.saturating_add(1);
        matched_positions = matched_positions.saturating_add(u32::from(score >= min_match_bps));
        score_sum = score_sum.saturating_add(u64::from(score));
        expected_fingerprints.push((prompt_id.as_str(), embedding_vector_fingerprint(expected)));
        observed_fingerprints.push((prompt_id.as_str(), embedding_vector_fingerprint(observed)));
    }

    let match_bps = if total_positions == 0 {
        0
    } else {
        (score_sum / u64::from(total_positions)) as u32
    };
    let expected_fingerprint = aggregate_canary_fingerprints(
        expected_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );
    let observed_fingerprint = aggregate_canary_fingerprints(
        observed_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );

    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_EMBEDDING_COSINE.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint,
        observed_fingerprint,
        matched_positions,
        total_positions,
        match_bps,
        pass: total_positions > 0 && matched_positions == total_positions,
    }
}

pub fn normalize_canary_transcript(value: &str) -> String {
    let mut normalized = String::new();
    let mut pending_space = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            pending_space = !normalized.is_empty();
        } else {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(ch);
            pending_space = false;
        }
    }
    normalized
}

fn text_fingerprint(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

pub fn evaluate_catalog_canary_transcript_match_probe(
    spec: &CanaryProbeSpec,
    expected_transcripts: &BTreeMap<String, String>,
    observed_transcripts: &BTreeMap<String, String>,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut expected_fingerprints = Vec::with_capacity(expected_transcripts.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_transcripts.len());

    for (prompt_id, expected) in expected_transcripts {
        let expected = normalize_canary_transcript(expected);
        let observed = observed_transcripts
            .get(prompt_id)
            .map(|value| normalize_canary_transcript(value))
            .unwrap_or_default();
        total_positions = total_positions.saturating_add(1);
        matched_positions = matched_positions
            .saturating_add(u32::from(!expected.is_empty() && expected == observed));
        expected_fingerprints.push((prompt_id.as_str(), text_fingerprint(&expected)));
        observed_fingerprints.push((prompt_id.as_str(), text_fingerprint(&observed)));
    }

    let match_bps = if total_positions == 0 {
        0
    } else {
        ((u64::from(matched_positions) * 10_000) / u64::from(total_positions)) as u32
    };
    let expected_fingerprint = aggregate_canary_fingerprints(
        expected_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );
    let observed_fingerprint = aggregate_canary_fingerprints(
        observed_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );

    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_TRANSCRIPT_MATCH.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint,
        observed_fingerprint,
        matched_positions,
        total_positions,
        match_bps,
        pass: total_positions > 0 && matched_positions == total_positions,
    }
}

pub fn audio_fingerprint(bytes: &[u8]) -> String {
    wav_audio_spectral_fingerprint(bytes)
        .map(|fingerprint| fingerprint.encode())
        .unwrap_or_else(|| blake3::hash(bytes).to_hex().to_string())
}

const AUDIO_FINGERPRINT_VERSION: &str = "audiospec-v1";
const AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE: u32 = 16_000;
const AUDIO_FINGERPRINT_FFT_SIZE: usize = 1_024;
const AUDIO_FINGERPRINT_HOP_SIZE: usize = 256;
const AUDIO_FINGERPRINT_BANDS: usize = 16;
const AUDIO_FINGERPRINT_TIME_BUCKETS: usize = 16;
const AUDIO_FINGERPRINT_VECTOR_LEN: usize =
    AUDIO_FINGERPRINT_BANDS * AUDIO_FINGERPRINT_TIME_BUCKETS;
const AUDIO_FINGERPRINT_QUANTIZATION_SCALE: f64 = 32.0;

#[derive(Clone, Debug, Eq, PartialEq)]
struct AudioSpectralFingerprint {
    duration_bucket_ms: u64,
    vector: Vec<i8>,
}

impl AudioSpectralFingerprint {
    fn encode(&self) -> String {
        let bytes = self
            .vector
            .iter()
            .map(|value| *value as u8)
            .collect::<Vec<_>>();
        format!(
            "{AUDIO_FINGERPRINT_VERSION}:{}:{}",
            self.duration_bucket_ms,
            hex::encode(bytes)
        )
    }

    fn decode(value: &str) -> Option<Self> {
        let mut fields = value.splitn(3, ':');
        if fields.next()? != AUDIO_FINGERPRINT_VERSION {
            return None;
        }
        let duration_bucket_ms = fields.next()?.parse::<u64>().ok()?;
        if duration_bucket_ms == 0 {
            return None;
        }
        let bytes = hex::decode(fields.next()?).ok()?;
        if bytes.len() != AUDIO_FINGERPRINT_VECTOR_LEN {
            return None;
        }
        let vector = bytes
            .into_iter()
            .map(|value| value as i8)
            .collect::<Vec<_>>();
        if vector.iter().all(|value| *value == 0) {
            return None;
        }
        Some(Self {
            duration_bucket_ms,
            vector,
        })
    }
}

fn wav_audio_spectral_fingerprint(bytes: &[u8]) -> Option<AudioSpectralFingerprint> {
    if bytes.len() < 44 || bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }

    let mut offset = 12_usize;
    let mut sample_rate = 0_u32;
    let mut channels = 0_u16;
    let mut bits_per_sample = 0_u16;
    let mut audio_format = 0_u16;
    let mut data = None;

    while offset.checked_add(8)? <= bytes.len() {
        let chunk_id = bytes.get(offset..offset + 4)?;
        let chunk_len =
            u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start.checked_add(chunk_len)?;
        if chunk_end > bytes.len() {
            return None;
        }
        match chunk_id {
            b"fmt " if chunk_len >= 16 => {
                audio_format =
                    u16::from_le_bytes(bytes.get(chunk_start..chunk_start + 2)?.try_into().ok()?);
                channels = u16::from_le_bytes(
                    bytes
                        .get(chunk_start + 2..chunk_start + 4)?
                        .try_into()
                        .ok()?,
                );
                sample_rate = u32::from_le_bytes(
                    bytes
                        .get(chunk_start + 4..chunk_start + 8)?
                        .try_into()
                        .ok()?,
                );
                bits_per_sample = u16::from_le_bytes(
                    bytes
                        .get(chunk_start + 14..chunk_start + 16)?
                        .try_into()
                        .ok()?,
                );
            }
            b"data" => data = Some(bytes.get(chunk_start..chunk_end)?),
            _ => {}
        }
        offset = chunk_end + (chunk_len & 1);
    }

    let data = data?;
    if sample_rate == 0 || channels == 0 || data.is_empty() {
        return None;
    }
    if audio_format != 1 || bits_per_sample != 16 {
        return None;
    }

    let frame_bytes = usize::from(channels).checked_mul(2)?;
    let frame_count = data.len() / frame_bytes;
    if frame_count == 0 {
        return None;
    }

    let mut mono = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let mut sum = 0_i32;
        for channel in 0..usize::from(channels) {
            let sample_offset = frame * frame_bytes + channel * 2;
            let sample = i16::from_le_bytes(
                data.get(sample_offset..sample_offset + 2)?
                    .try_into()
                    .ok()?,
            );
            sum += i32::from(sample);
        }
        mono.push(f64::from(sum) / f64::from(channels) / f64::from(i16::MAX));
    }

    let resampled_len = usize::try_from(
        (frame_count as u128).checked_mul(u128::from(AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE))?
            / u128::from(sample_rate),
    )
    .ok()?;
    if resampled_len == 0 {
        return None;
    }
    let mut resampled = Vec::with_capacity(resampled_len);
    for target_index in 0..resampled_len {
        let source_position_numerator =
            (target_index as u128).checked_mul(u128::from(sample_rate))?;
        let source_index = usize::try_from(
            source_position_numerator / u128::from(AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE),
        )
        .ok()?
        .min(frame_count - 1);
        let next_index = source_index.saturating_add(1).min(frame_count - 1);
        let fraction = (source_position_numerator
            % u128::from(AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE)) as f64
            / f64::from(AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE);
        resampled.push(mono[source_index] * (1.0 - fraction) + mono[next_index] * fraction);
    }
    let rms_squared =
        resampled.iter().map(|sample| sample * sample).sum::<f64>() / resampled.len() as f64;
    if !rms_squared.is_finite() || rms_squared <= f64::EPSILON {
        return None;
    }
    let rms = rms_squared.sqrt();
    for sample in &mut resampled {
        *sample /= rms;
    }

    let frame_windows = if resampled.len() <= AUDIO_FINGERPRINT_FFT_SIZE {
        1
    } else {
        1 + (resampled.len() - AUDIO_FINGERPRINT_FFT_SIZE) / AUDIO_FINGERPRINT_HOP_SIZE
    };
    let low_hz = 60.0_f64;
    let high_hz = (f64::from(AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE) * 0.4875).max(low_hz + 1.0);
    let band_ratio = (high_hz / low_hz).powf(1.0 / AUDIO_FINGERPRINT_BANDS as f64);
    let mut band_edges = Vec::with_capacity(AUDIO_FINGERPRINT_BANDS + 1);
    for index in 0..=AUDIO_FINGERPRINT_BANDS {
        band_edges.push(low_hz * band_ratio.powf(index as f64));
    }
    let mut bin_bands = vec![None; AUDIO_FINGERPRINT_FFT_SIZE / 2 + 1];
    let mut band_bin_counts = [0_u32; AUDIO_FINGERPRINT_BANDS];
    for (bin, target) in bin_bands.iter_mut().enumerate().skip(1) {
        let frequency = bin as f64 * f64::from(AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE)
            / AUDIO_FINGERPRINT_FFT_SIZE as f64;
        let band = (0..AUDIO_FINGERPRINT_BANDS)
            .find(|band| frequency >= band_edges[*band] && frequency < band_edges[*band + 1]);
        *target = band;
        if let Some(band) = band {
            band_bin_counts[band] = band_bin_counts[band].saturating_add(1);
        }
    }
    if band_bin_counts.iter().any(|count| *count == 0) {
        return None;
    }

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(AUDIO_FINGERPRINT_FFT_SIZE);
    let mut fft_buffer = vec![Complex::new(0.0, 0.0); AUDIO_FINGERPRINT_FFT_SIZE];
    let mut matrix = vec![0.0_f64; AUDIO_FINGERPRINT_VECTOR_LEN];
    let mut bucket_frames = [0_u32; AUDIO_FINGERPRINT_TIME_BUCKETS];
    let denominator = (AUDIO_FINGERPRINT_FFT_SIZE - 1) as f64;
    for frame in 0..frame_windows {
        let start = frame.saturating_mul(AUDIO_FINGERPRINT_HOP_SIZE);
        for (index, value) in fft_buffer.iter_mut().enumerate() {
            let sample = resampled.get(start + index).copied().unwrap_or(0.0);
            let window = 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / denominator).cos();
            *value = Complex::new(sample * window, 0.0);
        }
        fft.process(&mut fft_buffer);
        let time_bucket = ((frame * AUDIO_FINGERPRINT_TIME_BUCKETS) / frame_windows)
            .min(AUDIO_FINGERPRINT_TIME_BUCKETS - 1);
        bucket_frames[time_bucket] = bucket_frames[time_bucket].saturating_add(1);
        let mut frame_energy = [0.0_f64; AUDIO_FINGERPRINT_BANDS];
        for (bin, band) in bin_bands.iter().enumerate() {
            if let Some(band) = band {
                frame_energy[*band] += fft_buffer[bin].norm_sqr();
            }
        }
        for band in 0..AUDIO_FINGERPRINT_BANDS {
            matrix[time_bucket * AUDIO_FINGERPRINT_BANDS + band] +=
                frame_energy[band] / f64::from(band_bin_counts[band]);
        }
    }
    if bucket_frames.iter().any(|count| *count == 0) {
        return None;
    }
    for time_bucket in 0..AUDIO_FINGERPRINT_TIME_BUCKETS {
        for band in 0..AUDIO_FINGERPRINT_BANDS {
            let index = time_bucket * AUDIO_FINGERPRINT_BANDS + band;
            matrix[index] = (matrix[index] / f64::from(bucket_frames[time_bucket])).ln_1p();
        }
    }

    let mean = matrix.iter().sum::<f64>() / matrix.len() as f64;
    let variance = matrix
        .iter()
        .map(|value| {
            let centered = value - mean;
            centered * centered
        })
        .sum::<f64>()
        / matrix.len() as f64;
    if !variance.is_finite() || variance <= f64::EPSILON {
        return None;
    }
    let standard_deviation = variance.sqrt();
    let vector = matrix
        .into_iter()
        .map(|value| {
            (((value - mean) / standard_deviation) * AUDIO_FINGERPRINT_QUANTIZATION_SCALE)
                .round()
                .clamp(f64::from(i8::MIN), f64::from(i8::MAX)) as i8
        })
        .collect::<Vec<_>>();
    if vector.iter().all(|value| *value == 0) {
        return None;
    }

    let duration_ms = ((frame_count as u128) * 1_000_u128 / u128::from(sample_rate)) as u64;
    let duration_bucket_ms = ((duration_ms + 50) / 100) * 100;
    if duration_bucket_ms == 0 {
        return None;
    }
    Some(AudioSpectralFingerprint {
        duration_bucket_ms,
        vector,
    })
}

pub fn audio_fingerprint_similarity_bps(expected: &str, observed: &str) -> Option<u32> {
    let expected = AudioSpectralFingerprint::decode(expected)?;
    let observed = AudioSpectralFingerprint::decode(observed)?;
    if expected.duration_bucket_ms != observed.duration_bucket_ms
        || expected.vector.len() != observed.vector.len()
    {
        return Some(0);
    }
    let (dot, expected_norm, observed_norm) = expected.vector.iter().zip(&observed.vector).fold(
        (0.0_f64, 0.0_f64, 0.0_f64),
        |(dot, expected_norm, observed_norm), (expected, observed)| {
            let expected = f64::from(*expected);
            let observed = f64::from(*observed);
            (
                dot + expected * observed,
                expected_norm + expected * expected,
                observed_norm + observed * observed,
            )
        },
    );
    if expected_norm == 0.0 || observed_norm == 0.0 {
        return None;
    }
    let cosine = dot / (expected_norm.sqrt() * observed_norm.sqrt());
    if !cosine.is_finite() {
        return None;
    }
    Some((cosine.clamp(0.0, 1.0) * 10_000.0).round() as u32)
}

pub fn valid_audio_fingerprint(value: &str) -> bool {
    AudioSpectralFingerprint::decode(value).is_some()
}

pub fn evaluate_catalog_canary_audio_fingerprint_probe(
    spec: &CanaryProbeSpec,
    expected_fingerprints_by_prompt: &BTreeMap<String, String>,
    observed_fingerprints_by_prompt: &BTreeMap<String, String>,
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut score_sum = 0_u64;
    let mut expected_fingerprints = Vec::with_capacity(expected_fingerprints_by_prompt.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_fingerprints_by_prompt.len());

    for (prompt_id, expected) in expected_fingerprints_by_prompt {
        let observed = observed_fingerprints_by_prompt
            .get(prompt_id)
            .map(String::as_str)
            .unwrap_or_default();
        let score = audio_fingerprint_similarity_bps(expected, observed).unwrap_or(0);
        total_positions = total_positions.saturating_add(1);
        matched_positions = matched_positions.saturating_add(u32::from(score >= min_match_bps));
        score_sum = score_sum.saturating_add(u64::from(score));
        expected_fingerprints.push((prompt_id.as_str(), expected.to_ascii_lowercase()));
        observed_fingerprints.push((prompt_id.as_str(), observed.to_ascii_lowercase()));
    }

    let match_bps = if total_positions == 0 {
        0
    } else {
        (score_sum / u64::from(total_positions)) as u32
    };
    let expected_fingerprint = aggregate_canary_fingerprints(
        expected_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );
    let observed_fingerprint = aggregate_canary_fingerprints(
        observed_fingerprints
            .iter()
            .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
    );

    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_AUDIO_FINGERPRINT.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint,
        observed_fingerprint,
        matched_positions,
        total_positions,
        match_bps,
        pass: total_positions > 0
            && matched_positions == total_positions
            && match_bps >= min_match_bps,
    }
}

const VIDEO_AV_FINGERPRINT_VERSION: &str = "videoav-v1";
const VIDEO_AV_MAX_MP4_BYTES: usize = 64 * 1024 * 1024;
const VIDEO_AV_MAX_SAMPLE_FRAMES: usize = 16;
const VIDEO_AV_FRAME_EDGE: usize = 16;
const VIDEO_AV_FRAME_BYTES: usize = VIDEO_AV_FRAME_EDGE * VIDEO_AV_FRAME_EDGE;
const VIDEO_AV_MAX_AUDIO_SECONDS: u64 = 30;
const VIDEO_AV_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct VideoAvFingerprint {
    width: u32,
    height: u32,
    frame_count: u64,
    fps_milli: u32,
    video_duration_ms: u64,
    audio_duration_ms: u64,
    frame_hashes: Vec<String>,
    audio_fingerprint: String,
}

impl VideoAvFingerprint {
    fn encode(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|err| format!("serializing video canary fingerprint failed: {err}"))?;
        Ok(format!(
            "{VIDEO_AV_FINGERPRINT_VERSION}:{}",
            hex::encode(bytes)
        ))
    }

    fn decode(value: &str) -> Option<Self> {
        let encoded = value.strip_prefix(&format!("{VIDEO_AV_FINGERPRINT_VERSION}:"))?;
        let fingerprint: Self = serde_json::from_slice(&hex::decode(encoded).ok()?).ok()?;
        fingerprint.valid().then_some(fingerprint)
    }

    fn valid(&self) -> bool {
        let one_frame_ms =
            (self.fps_milli > 0).then(|| 1_000_000_u64.div_ceil(u64::from(self.fps_milli)));
        self.width > 0
            && self.height > 0
            && self.width <= 16_384
            && self.height <= 16_384
            && self.frame_count > 0
            && self.frame_count <= 1_000_000
            && self.fps_milli > 0
            && self.fps_milli <= 240_000
            && self.video_duration_ms > 0
            && self.audio_duration_ms > 0
            && one_frame_ms.is_some_and(|frame_ms| {
                self.video_duration_ms.abs_diff(self.audio_duration_ms)
                    <= frame_ms.saturating_add(100)
            })
            && !self.frame_hashes.is_empty()
            && self.frame_hashes.len() <= VIDEO_AV_MAX_SAMPLE_FRAMES
            && self.frame_hashes.iter().all(|hash| {
                hash.len() == VIDEO_AV_FRAME_BYTES / 4
                    && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            && valid_audio_fingerprint(&self.audio_fingerprint)
    }
}

#[derive(Debug, Deserialize)]
struct VideoAvProbeOutput {
    #[serde(default)]
    streams: Vec<VideoAvProbeStream>,
}

#[derive(Debug, Deserialize)]
struct VideoAvProbeStream {
    codec_type: String,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    nb_frames: Option<String>,
    #[serde(default)]
    nb_read_frames: Option<String>,
}

struct CanaryTempFile {
    path: PathBuf,
}

impl CanaryTempFile {
    fn create(label: &str, bytes: &[u8]) -> Result<Self, String> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random)
            .map_err(|err| format!("generating video canary temporary name failed: {err}"))?;
        let path = env::temp_dir().join(format!(
            "mayhem-video-canary-{label}-{}-{}",
            std::process::id(),
            hex::encode(random)
        ));
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|err| format!("creating video canary temporary file failed: {err}"))?;
        file.write_all(bytes)
            .map_err(|err| format!("writing video canary temporary file failed: {err}"))?;
        Ok(Self { path })
    }
}

impl Drop for CanaryTempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn video_canary_tool(environment: &str, fallback: &str) -> std::ffi::OsString {
    env::var_os(environment).unwrap_or_else(|| fallback.into())
}

fn run_video_canary_tool(
    program: std::ffi::OsString,
    args: &[String],
    max_stdout_bytes: usize,
) -> Result<Vec<u8>, String> {
    let stdout = CanaryTempFile::create("stdout", &[])?;
    let stderr = CanaryTempFile::create("stderr", &[])?;
    let stdout_file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&stdout.path)
        .map_err(|err| format!("opening video canary stdout failed: {err}"))?;
    let stderr_file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&stderr.path)
        .map_err(|err| format!("opening video canary stderr failed: {err}"))?;
    let mut child = Command::new(&program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|err| {
            format!(
                "starting video canary tool {} failed: {err}",
                Path::new(&program).display()
            )
        })?;
    let status = match child
        .wait_timeout(VIDEO_AV_TOOL_TIMEOUT)
        .map_err(|err| format!("waiting for video canary tool failed: {err}"))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "video canary tool {} exceeded {}s",
                Path::new(&program).display(),
                VIDEO_AV_TOOL_TIMEOUT.as_secs()
            ));
        }
    };
    let stderr_bytes = fs::read(&stderr.path)
        .map_err(|err| format!("reading video canary stderr failed: {err}"))?;
    if !status.success() {
        return Err(format!(
            "video canary tool {} failed: {}",
            Path::new(&program).display(),
            String::from_utf8_lossy(&stderr_bytes[..stderr_bytes.len().min(4_096)])
        ));
    }
    let length = fs::metadata(&stdout.path)
        .map_err(|err| format!("reading video canary output metadata failed: {err}"))?
        .len();
    if length == 0 || length > max_stdout_bytes as u64 {
        return Err(format!(
            "video canary tool output must contain 1..={max_stdout_bytes} bytes, got {length}"
        ));
    }
    fs::read(&stdout.path).map_err(|err| format!("reading video canary output failed: {err}"))
}

fn parse_fraction(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    (numerator.is_finite() && denominator.is_finite() && numerator > 0.0 && denominator > 0.0)
        .then_some(numerator / denominator)
}

fn parse_seconds_millis(value: &str) -> Option<u64> {
    let seconds = value.parse::<f64>().ok()?;
    (seconds.is_finite() && seconds > 0.0 && seconds <= 3_600.0)
        .then(|| (seconds * 1_000.0).round().max(1.0) as u64)
}

fn video_frame_average_hash(frame: &[u8]) -> String {
    let average =
        frame.iter().map(|value| u64::from(*value)).sum::<u64>() / frame.len().max(1) as u64;
    let mut bits = vec![0_u8; VIDEO_AV_FRAME_BYTES / 8];
    for (index, value) in frame.iter().enumerate() {
        if u64::from(*value) >= average {
            bits[index / 8] |= 1 << (7 - index % 8);
        }
    }
    hex::encode(bits)
}

fn pcm16_mono_wav(samples: &[u8], sample_rate: u32) -> Result<Vec<u8>, String> {
    if samples.is_empty() || samples.len() % 2 != 0 {
        return Err("decoded video canary audio is not PCM16".to_owned());
    }
    let data_len = u32::try_from(samples.len())
        .map_err(|_| "decoded video canary audio exceeds WAV bounds".to_owned())?;
    let mut wav = Vec::with_capacity(samples.len().saturating_add(44));
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&sample_rate.saturating_mul(2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(samples);
    Ok(wav)
}

pub fn video_av_fingerprint(mp4: &[u8]) -> Result<String, String> {
    if mp4.len() < 16 || mp4.len() > VIDEO_AV_MAX_MP4_BYTES || mp4.get(4..8) != Some(b"ftyp") {
        return Err(format!(
            "video canary must be a bounded MP4 of 16..={VIDEO_AV_MAX_MP4_BYTES} bytes"
        ));
    }
    let input = CanaryTempFile::create("input.mp4", mp4)?;
    let input_path = input.path.display().to_string();
    let probe = run_video_canary_tool(
        video_canary_tool("MAYHEM_CANARY_FFPROBE", "ffprobe"),
        &[
            "-v".to_owned(),
            "error".to_owned(),
            "-count_frames".to_owned(),
            "-show_entries".to_owned(),
            "stream=codec_type,width,height,avg_frame_rate,duration,nb_frames,nb_read_frames"
                .to_owned(),
            "-of".to_owned(),
            "json".to_owned(),
            input_path.clone(),
        ],
        64 * 1024,
    )?;
    let probe: VideoAvProbeOutput = serde_json::from_slice(&probe)
        .map_err(|err| format!("parsing video canary ffprobe output failed: {err}"))?;
    let video = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type == "video")
        .ok_or_else(|| "video canary MP4 has no video stream".to_owned())?;
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type == "audio")
        .ok_or_else(|| "video canary MP4 has no audio stream".to_owned())?;
    let width = video
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| "video canary MP4 has no positive decoded width".to_owned())?;
    let height = video
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| "video canary MP4 has no positive decoded height".to_owned())?;
    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(parse_fraction)
        .filter(|value| *value <= 240.0)
        .ok_or_else(|| "video canary MP4 has no bounded frame rate".to_owned())?;
    let fps_milli = (fps * 1_000.0).round().max(1.0) as u32;
    let video_duration_ms = video
        .duration
        .as_deref()
        .and_then(parse_seconds_millis)
        .ok_or_else(|| "video canary MP4 has no bounded video duration".to_owned())?;
    let audio_duration_ms = audio
        .duration
        .as_deref()
        .and_then(parse_seconds_millis)
        .ok_or_else(|| "video canary MP4 has no bounded audio duration".to_owned())?;
    let frame_count = video
        .nb_read_frames
        .as_deref()
        .or(video.nb_frames.as_deref())
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| "video canary MP4 has no positive decoded frame count".to_owned())?;
    let stride = frame_count
        .div_ceil(VIDEO_AV_MAX_SAMPLE_FRAMES as u64)
        .max(1);
    let raw_frames = run_video_canary_tool(
        video_canary_tool("MAYHEM_CANARY_FFMPEG", "ffmpeg"),
        &[
            "-v".to_owned(),
            "error".to_owned(),
            "-nostdin".to_owned(),
            "-i".to_owned(),
            input_path.clone(),
            "-map".to_owned(),
            "0:v:0".to_owned(),
            "-an".to_owned(),
            "-vf".to_owned(),
            format!(
                "select=not(mod(n\\,{stride})),scale={VIDEO_AV_FRAME_EDGE}:{VIDEO_AV_FRAME_EDGE}:flags=area,format=gray"
            ),
            "-frames:v".to_owned(),
            VIDEO_AV_MAX_SAMPLE_FRAMES.to_string(),
            "-fps_mode".to_owned(),
            "vfr".to_owned(),
            "-f".to_owned(),
            "rawvideo".to_owned(),
            "-pix_fmt".to_owned(),
            "gray".to_owned(),
            "-".to_owned(),
        ],
        VIDEO_AV_FRAME_BYTES * VIDEO_AV_MAX_SAMPLE_FRAMES,
    )?;
    if raw_frames.len() % VIDEO_AV_FRAME_BYTES != 0 {
        return Err("video canary decoded frame output is truncated".to_owned());
    }
    let frame_hashes = raw_frames
        .chunks_exact(VIDEO_AV_FRAME_BYTES)
        .map(video_frame_average_hash)
        .collect::<Vec<_>>();
    if frame_hashes.is_empty() {
        return Err("video canary MP4 decoded no video frames".to_owned());
    }
    let pcm = run_video_canary_tool(
        video_canary_tool("MAYHEM_CANARY_FFMPEG", "ffmpeg"),
        &[
            "-v".to_owned(),
            "error".to_owned(),
            "-nostdin".to_owned(),
            "-i".to_owned(),
            input_path,
            "-map".to_owned(),
            "0:a:0".to_owned(),
            "-vn".to_owned(),
            "-ac".to_owned(),
            "1".to_owned(),
            "-ar".to_owned(),
            AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE.to_string(),
            "-t".to_owned(),
            VIDEO_AV_MAX_AUDIO_SECONDS.to_string(),
            "-f".to_owned(),
            "s16le".to_owned(),
            "-".to_owned(),
        ],
        (AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE as usize)
            .saturating_mul(2)
            .saturating_mul(VIDEO_AV_MAX_AUDIO_SECONDS as usize),
    )?;
    let samples = pcm
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    let active_samples = samples
        .iter()
        .filter(|sample| sample.unsigned_abs() > 8)
        .count();
    let mean_square = samples
        .iter()
        .map(|sample| {
            let sample = f64::from(*sample);
            sample * sample
        })
        .sum::<f64>()
        / samples.len().max(1) as f64;
    if active_samples.saturating_mul(200) < samples.len() || mean_square.sqrt() <= 4.0 {
        return Err("video canary MP4 audio is silent".to_owned());
    }
    let audio_fingerprint =
        audio_fingerprint(&pcm16_mono_wav(&pcm, AUDIO_FINGERPRINT_TARGET_SAMPLE_RATE)?);
    if !valid_audio_fingerprint(&audio_fingerprint) {
        return Err("video canary MP4 audio has no stable spectral fingerprint".to_owned());
    }
    let fingerprint = VideoAvFingerprint {
        width,
        height,
        frame_count,
        fps_milli,
        video_duration_ms,
        audio_duration_ms,
        frame_hashes,
        audio_fingerprint,
    };
    if !fingerprint.valid() {
        return Err("video canary MP4 is not synchronized within one frame".to_owned());
    }
    fingerprint.encode()
}

pub fn valid_video_av_fingerprint(value: &str) -> bool {
    VideoAvFingerprint::decode(value).is_some()
}

pub fn video_av_fingerprint_similarity_bps(expected: &str, observed: &str) -> Option<u32> {
    let expected = VideoAvFingerprint::decode(expected)?;
    let observed = VideoAvFingerprint::decode(observed)?;
    if expected.width != observed.width
        || expected.height != observed.height
        || expected.frame_hashes.len() != observed.frame_hashes.len()
    {
        return Some(0);
    }
    let frame_score = expected
        .frame_hashes
        .iter()
        .zip(&observed.frame_hashes)
        .filter_map(|(expected, observed)| perceptual_hash_match_stats(expected, observed))
        .map(|(_, _, score)| u64::from(score))
        .sum::<u64>()
        / expected.frame_hashes.len().max(1) as u64;
    let audio_score = u64::from(
        audio_fingerprint_similarity_bps(&expected.audio_fingerprint, &observed.audio_fingerprint)
            .unwrap_or(0),
    );
    let one_frame_ms =
        1_000_000_u64.div_ceil(u64::from(expected.fps_milli.min(observed.fps_milli)));
    let structure_matches = expected.frame_count.abs_diff(observed.frame_count) <= 1
        && expected.fps_milli.abs_diff(observed.fps_milli) <= 100
        && expected
            .video_duration_ms
            .abs_diff(observed.video_duration_ms)
            <= one_frame_ms.saturating_add(100)
        && expected
            .audio_duration_ms
            .abs_diff(observed.audio_duration_ms)
            <= one_frame_ms.saturating_add(100);
    let structure_score = u64::from(structure_matches) * 10_000;
    let aggregate =
        ((frame_score * 7_000 + audio_score * 2_500 + structure_score * 500) / 10_000).min(10_000);
    Some(aggregate.min(audio_score) as u32)
}

pub fn evaluate_catalog_canary_video_av_fingerprint_probe(
    spec: &CanaryProbeSpec,
    expected_fingerprints_by_prompt: &BTreeMap<String, String>,
    observed_fingerprints_by_prompt: &BTreeMap<String, String>,
    min_match_bps: u32,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut score_sum = 0_u64;
    let mut expected_fingerprints = Vec::with_capacity(expected_fingerprints_by_prompt.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_fingerprints_by_prompt.len());
    for (prompt_id, expected) in expected_fingerprints_by_prompt {
        let observed = observed_fingerprints_by_prompt
            .get(prompt_id)
            .map(String::as_str)
            .unwrap_or_default();
        let score = video_av_fingerprint_similarity_bps(expected, observed).unwrap_or(0);
        total_positions = total_positions.saturating_add(1);
        matched_positions = matched_positions.saturating_add(u32::from(score >= min_match_bps));
        score_sum = score_sum.saturating_add(u64::from(score));
        expected_fingerprints.push((prompt_id.as_str(), expected.to_ascii_lowercase()));
        observed_fingerprints.push((prompt_id.as_str(), observed.to_ascii_lowercase()));
    }
    let match_bps = if total_positions == 0 {
        0
    } else {
        (score_sum / u64::from(total_positions)) as u32
    };
    CanaryProbeEvaluation {
        verification_method: CANARY_VERIFICATION_VIDEO_AV_FINGERPRINT.to_owned(),
        canary_set: spec.canary_set.clone(),
        prompt_id: spec.prompt_id.clone(),
        expected_fingerprint: aggregate_canary_fingerprints(
            expected_fingerprints
                .iter()
                .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
        ),
        observed_fingerprint: aggregate_canary_fingerprints(
            observed_fingerprints
                .iter()
                .map(|(prompt_id, fingerprint)| (*prompt_id, fingerprint.as_str())),
        ),
        matched_positions,
        total_positions,
        match_bps,
        pass: total_positions > 0
            && matched_positions == total_positions
            && match_bps >= min_match_bps,
    }
}

pub fn perceptual_hash_match_stats(
    expected_hash: &str,
    observed_hash: &str,
) -> Option<(u32, u32, u32)> {
    if expected_hash.is_empty() || expected_hash.len() != observed_hash.len() {
        return None;
    }
    let mut matched_bits = 0_u32;
    let mut total_bits = 0_u32;
    for (expected, observed) in expected_hash.bytes().zip(observed_hash.bytes()) {
        let expected = hex_nibble(expected)?;
        let observed = hex_nibble(observed)?;
        let distance = (expected ^ observed).count_ones();
        matched_bits = matched_bits.saturating_add(4_u32.saturating_sub(distance));
        total_bits = total_bits.saturating_add(4);
    }
    if total_bits == 0 {
        return None;
    }
    let match_bps = ((u64::from(matched_bits) * 10_000) / u64::from(total_bits)) as u32;
    Some((matched_bits, total_bits, match_bps))
}

pub fn image_average_hash_hex(bytes: &[u8]) -> Result<String, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|err| format!("invalid image artifact for perceptual hash: {err}"))?;
    let gray = image
        .resize_exact(8, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let pixels = gray.as_raw();
    if pixels.is_empty() {
        return Err("image artifact has no pixels".to_owned());
    }
    let sum = pixels.iter().map(|pixel| u32::from(*pixel)).sum::<u32>();
    let average = f64::from(sum) / pixels.len() as f64;
    let mut bits = 0_u64;
    for pixel in pixels {
        bits <<= 1;
        if f64::from(*pixel) >= average {
            bits |= 1;
        }
    }
    Ok(format!("{bits:016x}"))
}

fn hex_nibble(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(byte - b'A' + 10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> CanaryProbeSpec {
        CanaryProbeSpec {
            model: "mayhem/dev-chat-tools".to_owned(),
            canary_set: "canary-dev-v1".to_owned(),
            prompt_id: "identity-check".to_owned(),
            prompt: "Return the fixed continuation.".to_owned(),
            seed: 7,
            max_tokens: 8,
            sampling: CanarySamplingProfile {
                temperature: Some(0.0),
                frequency_penalty: Some(-0.25),
                presence_penalty: Some(1.5),
                ..CanarySamplingProfile::default()
            },
        }
    }

    #[test]
    fn canary_probe_request_is_regular_paid_openai_session_shape() {
        let body = spec().openai_chat_body();
        assert_eq!(body["model"], "mayhem/dev-chat-tools");
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["frequency_penalty"], -0.25);
        assert_eq!(body["presence_penalty"], 1.5);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);

        let serialized = body.to_string();
        assert!(!serialized.contains("audit"));
        assert!(!serialized.contains("canary"));
    }

    #[test]
    fn canary_match_scores_positions_and_detects_missealed_output() {
        let spec = spec();
        let expected = [10, 20, 30, 40, 50];
        let observed = [10, 20, 999, 40, 998];
        let evaluation =
            evaluate_canary_probe(&spec, &expected, &observed, DEFAULT_CANARY_MATCH_MIN_BPS);

        assert_eq!(evaluation.matched_positions, 3);
        assert_eq!(evaluation.total_positions, 5);
        assert_eq!(evaluation.match_bps, 6_000);
        assert!(!evaluation.pass);
        assert_eq!(
            evaluation.verification_method,
            CANARY_VERIFICATION_TOKEN_FINGERPRINT
        );
        assert!(evaluation.provenance_violation());
        assert_eq!(
            evaluation.reputation_event_kind(),
            ReputationEventKind::ProbeFail
        );
        assert_ne!(
            evaluation.expected_fingerprint,
            evaluation.observed_fingerprint
        );
    }

    #[test]
    fn canary_match_passes_at_threshold() {
        let expected = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let observed = [1, 2, 3, 4, 5, 6, 7, 8, 9, 99];
        let evaluation = evaluate_canary_probe(&spec(), &expected, &observed, 9_000);

        assert_eq!(evaluation.match_bps, 9_000);
        assert!(evaluation.pass);
        assert_eq!(
            evaluation.reputation_event_kind(),
            ReputationEventKind::ProbeOk
        );
    }

    #[test]
    fn aggregate_canary_fingerprint_matches_catalog_format() {
        let first = token_fingerprint([1, 2, 3]).digest;
        let second = token_fingerprint([4, 5]).digest;
        let aggregate =
            aggregate_canary_fingerprints([("a", first.as_str()), ("b", second.as_str())]);

        assert_eq!(
            aggregate,
            aggregate_canary_fingerprints([("a", first.as_str()), ("b", second.as_str())])
        );
        assert_ne!(
            aggregate,
            aggregate_canary_fingerprints([("b", second.as_str()), ("a", first.as_str())])
        );
    }

    #[test]
    fn catalog_fingerprint_evaluation_emits_probe_fail_on_mismatch() {
        let evaluation = evaluate_catalog_canary_probe(&spec(), "aa", "bb", 9_000);

        assert_eq!(evaluation.match_bps, 0);
        assert!(!evaluation.pass);
        assert_eq!(
            evaluation.reputation_event_kind(),
            ReputationEventKind::ProbeFail
        );

        let ok = evaluate_catalog_canary_probe(&spec(), "AA", "aa", 9_000);
        assert_eq!(ok.match_bps, 10_000);
        assert!(ok.pass);
    }

    #[test]
    fn catalog_token_prefix_evaluation_requires_exact_prefixes() {
        let expected = BTreeMap::from([
            ("first".to_owned(), vec![1, 2, 3]),
            ("second".to_owned(), vec![4, 5]),
        ]);
        let observed_ok = BTreeMap::from([
            ("first".to_owned(), vec![1, 2, 3, 999]),
            ("second".to_owned(), vec![4, 5, 999]),
        ]);
        let ok = evaluate_catalog_canary_token_prefix_probe(&spec(), &expected, &observed_ok);
        assert_eq!(ok.match_bps, 10_000);
        assert!(ok.pass);

        let observed_swap = BTreeMap::from([
            ("first".to_owned(), vec![1, 2, 99]),
            ("second".to_owned(), vec![4, 5]),
        ]);
        let mismatch =
            evaluate_catalog_canary_token_prefix_probe(&spec(), &expected, &observed_swap);
        assert_eq!(mismatch.matched_positions, 4);
        assert_eq!(mismatch.total_positions, 5);
        assert_eq!(mismatch.match_bps, 8_000);
        assert!(!mismatch.pass);
    }

    #[test]
    fn seed_perceptual_hash_evaluation_uses_hamming_tolerance() {
        let near = evaluate_seed_perceptual_hash_probe(&spec(), "ffff", "fffe", 9_000);
        assert_eq!(
            near.verification_method,
            CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH
        );
        assert_eq!(near.matched_positions, 15);
        assert_eq!(near.total_positions, 16);
        assert_eq!(near.match_bps, 9_375);
        assert!(near.pass);

        let far = evaluate_seed_perceptual_hash_probe(&spec(), "ffff", "0000", 9_000);
        assert_eq!(far.match_bps, 0);
        assert!(!far.pass);

        let malformed = evaluate_seed_perceptual_hash_probe(&spec(), "ffff", "not-hex", 9_000);
        assert_eq!(malformed.match_bps, 0);
        assert!(!malformed.pass);
    }

    #[test]
    fn catalog_perceptual_hash_evaluation_aggregates_prompt_hashes() {
        let expected = BTreeMap::from([
            ("first".to_owned(), "ffff".to_owned()),
            ("second".to_owned(), "0000".to_owned()),
        ]);
        let observed = BTreeMap::from([
            ("first".to_owned(), "fffe".to_owned()),
            ("second".to_owned(), "0000".to_owned()),
        ]);

        let ok =
            evaluate_catalog_canary_perceptual_hash_probe(&spec(), &expected, &observed, 9_000);
        assert_eq!(ok.matched_positions, 31);
        assert_eq!(ok.total_positions, 32);
        assert_eq!(ok.match_bps, 9_687);
        assert!(ok.pass);

        let missing = BTreeMap::from([("first".to_owned(), "fffe".to_owned())]);
        let fail =
            evaluate_catalog_canary_perceptual_hash_probe(&spec(), &expected, &missing, 9_000);
        assert_eq!(fail.total_positions, 32);
        assert!(!fail.pass);
    }

    #[test]
    fn transcript_match_preserves_case_and_punctuation_but_collapses_whitespace() {
        let expected = BTreeMap::from([(
            "fixed".to_owned(),
            "Hello, world!  Mayhem works.".to_owned(),
        )]);
        let whitespace_only = BTreeMap::from([(
            "fixed".to_owned(),
            "  Hello, world!\nMayhem works.  ".to_owned(),
        )]);
        assert!(
            evaluate_catalog_canary_transcript_match_probe(&spec(), &expected, &whitespace_only)
                .pass
        );

        for observed in ["hello, world! Mayhem works.", "Hello world Mayhem works"] {
            let mismatch = BTreeMap::from([("fixed".to_owned(), observed.to_owned())]);
            assert!(
                !evaluate_catalog_canary_transcript_match_probe(&spec(), &expected, &mismatch).pass
            );
        }
    }

    fn test_pcm16_wav(frequency_hz: f64, gain: f64, duration_seconds: u32) -> Vec<u8> {
        let sample_rate = 16_000_u32;
        let frames = sample_rate * duration_seconds;
        let data_len = frames * 2;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for frame in 0..frames {
            let time = f64::from(frame) / f64::from(sample_rate);
            let envelope = 0.55 + 0.45 * (std::f64::consts::TAU * 1.75 * time).sin().abs();
            let sample = gain
                * envelope
                * ((std::f64::consts::TAU * frequency_hz * time).sin()
                    + 0.3 * (std::f64::consts::TAU * frequency_hz * 1.5 * time).sin());
            let sample = (sample * f64::from(i16::MAX))
                .round()
                .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    #[test]
    fn audio_spectral_fingerprint_is_gain_invariant_and_content_sensitive() {
        let reference = audio_fingerprint(&test_pcm16_wav(220.0, 0.5, 2));
        let gain_variant = audio_fingerprint(&test_pcm16_wav(220.0, 0.2, 2));
        let unrelated = audio_fingerprint(&test_pcm16_wav(880.0, 0.5, 2));

        assert!(valid_audio_fingerprint(&reference));
        assert_eq!(
            audio_fingerprint_similarity_bps(&reference, &gain_variant),
            Some(10_000)
        );
        assert!(
            audio_fingerprint_similarity_bps(&reference, &unrelated).unwrap_or_default() < 9_000
        );
        assert!(!valid_audio_fingerprint(&audio_fingerprint(
            &test_pcm16_wav(220.0, 0.0, 2)
        )));
    }

    #[test]
    fn audio_spectral_probe_applies_tolerance_and_rejects_duration_drift() {
        let expected_fingerprint = audio_fingerprint(&test_pcm16_wav(220.0, 0.5, 2));
        let expected = BTreeMap::from([("fixed".to_owned(), expected_fingerprint.clone())]);
        let exact = BTreeMap::from([("fixed".to_owned(), expected_fingerprint)]);
        let exact_evaluation =
            evaluate_catalog_canary_audio_fingerprint_probe(&spec(), &expected, &exact, 9_000);
        assert_eq!(exact_evaluation.match_bps, 10_000);
        assert!(exact_evaluation.pass);

        let wrong_duration = BTreeMap::from([(
            "fixed".to_owned(),
            audio_fingerprint(&test_pcm16_wav(220.0, 0.5, 3)),
        )]);
        let mismatch = evaluate_catalog_canary_audio_fingerprint_probe(
            &spec(),
            &expected,
            &wrong_duration,
            9_000,
        );
        assert_eq!(mismatch.match_bps, 0);
        assert!(!mismatch.pass);
    }

    fn test_video_av_fingerprint(frame_hashes: Vec<String>, audio_hz: f64) -> String {
        VideoAvFingerprint {
            width: 512,
            height: 320,
            frame_count: 9,
            fps_milli: 24_000,
            video_duration_ms: 375,
            audio_duration_ms: 375,
            frame_hashes,
            audio_fingerprint: audio_fingerprint(&test_pcm16_wav(audio_hz, 0.5, 2)),
        }
        .encode()
        .expect("encode video A/V fingerprint")
    }

    fn test_mp4(audio_source: &str, metadata: &str) -> Vec<u8> {
        run_video_canary_tool(
            video_canary_tool("MAYHEM_CANARY_FFMPEG", "ffmpeg"),
            &[
                "-v".to_owned(),
                "error".to_owned(),
                "-nostdin".to_owned(),
                "-f".to_owned(),
                "lavfi".to_owned(),
                "-i".to_owned(),
                "testsrc2=size=96x64:rate=8:duration=1".to_owned(),
                "-f".to_owned(),
                "lavfi".to_owned(),
                "-i".to_owned(),
                audio_source.to_owned(),
                "-frames:v".to_owned(),
                "9".to_owned(),
                "-t".to_owned(),
                "1.125".to_owned(),
                "-c:v".to_owned(),
                "mpeg4".to_owned(),
                "-q:v".to_owned(),
                "4".to_owned(),
                "-c:a".to_owned(),
                "aac".to_owned(),
                "-metadata".to_owned(),
                format!("comment={metadata}"),
                "-movflags".to_owned(),
                "frag_keyframe+empty_moov".to_owned(),
                "-f".to_owned(),
                "mp4".to_owned(),
                "-".to_owned(),
            ],
            4 * 1024 * 1024,
        )
        .expect("generate bounded MP4 fixture")
    }

    #[test]
    fn video_av_fingerprint_decodes_real_mp4_and_ignores_container_metadata() {
        let reference = test_mp4(
            "sine=frequency=220:sample_rate=16000:duration=1.125",
            "first",
        );
        let remuxed = test_mp4(
            "sine=frequency=220:sample_rate=16000:duration=1.125",
            "second",
        );
        assert_ne!(reference, remuxed);

        let expected = video_av_fingerprint(&reference).expect("reference MP4 fingerprint");
        let observed = video_av_fingerprint(&remuxed).expect("remuxed MP4 fingerprint");
        assert!(valid_video_av_fingerprint(&expected));
        assert!(video_av_fingerprint_similarity_bps(&expected, &observed)
            .is_some_and(|score| score >= 9_000));
    }

    #[test]
    fn video_av_fingerprint_rejects_silent_mp4_audio() {
        let silent = test_mp4(
            "anullsrc=channel_layout=mono:sample_rate=16000:duration=1.125",
            "silent",
        );
        assert!(video_av_fingerprint(&silent)
            .unwrap_err()
            .contains("audio is silent"));
    }

    #[test]
    fn video_av_fingerprint_tolerates_small_visual_drift_but_rejects_content_mismatch() {
        let base_hash = "0f".repeat(VIDEO_AV_FRAME_BYTES / 8);
        let mut drifted_bytes = hex::decode(&base_hash).expect("base frame hash");
        drifted_bytes[0] ^= 0b0000_0011;
        let drifted_hash = hex::encode(drifted_bytes);
        let expected = test_video_av_fingerprint(vec![base_hash; 3], 220.0);
        let tolerated = test_video_av_fingerprint(vec![drifted_hash; 3], 220.0);
        let mismatch = test_video_av_fingerprint(vec!["f0".repeat(32); 3], 880.0);

        assert!(valid_video_av_fingerprint(&expected));
        assert!(video_av_fingerprint_similarity_bps(&expected, &tolerated)
            .is_some_and(|score| score >= 9_000));
        assert!(video_av_fingerprint_similarity_bps(&expected, &mismatch)
            .is_some_and(|score| score < 9_000));
    }

    #[test]
    fn video_av_probe_rejects_audio_substitution_with_exact_video_and_structure() {
        let expected_value = test_video_av_fingerprint(vec!["0f".repeat(32); 3], 220.0);
        let mut substituted =
            VideoAvFingerprint::decode(&expected_value).expect("decode expected video fingerprint");
        let mut audio = AudioSpectralFingerprint::decode(&substituted.audio_fingerprint)
            .expect("decode expected audio fingerprint");
        for value in &mut audio.vector {
            *value = if *value == i8::MIN { i8::MAX } else { -*value };
        }
        substituted.audio_fingerprint = audio.encode();
        let substituted_value = substituted.encode().expect("encode substituted audio");

        assert!(
            video_av_fingerprint_similarity_bps(&expected_value, &substituted_value)
                .is_some_and(|score| score < 7_500),
            "independently wrong audio must cap the joint similarity below tolerance"
        );
        let expected = BTreeMap::from([("fixed".to_owned(), expected_value)]);
        let observed = BTreeMap::from([("fixed".to_owned(), substituted_value)]);
        let evaluation = evaluate_catalog_canary_video_av_fingerprint_probe(
            &spec(),
            &expected,
            &observed,
            7_500,
        );
        assert!(!evaluation.pass);
        assert_eq!(evaluation.matched_positions, 0);
    }

    #[test]
    fn video_av_probe_requires_every_prompt_and_detects_mismatch() {
        let expected_value = test_video_av_fingerprint(vec!["0f".repeat(32); 2], 220.0);
        let mismatch_value = test_video_av_fingerprint(vec!["f0".repeat(32); 2], 880.0);
        let expected = BTreeMap::from([
            ("t2v".to_owned(), expected_value.clone()),
            ("i2v".to_owned(), expected_value.clone()),
        ]);
        let observed = BTreeMap::from([
            ("t2v".to_owned(), expected_value),
            ("i2v".to_owned(), mismatch_value),
        ]);
        let evaluation = evaluate_catalog_canary_video_av_fingerprint_probe(
            &spec(),
            &expected,
            &observed,
            9_000,
        );

        assert!(!evaluation.pass);
        assert_eq!(
            evaluation.verification_method,
            CANARY_VERIFICATION_VIDEO_AV_FINGERPRINT
        );
        assert_eq!(evaluation.matched_positions, 1);
        assert_eq!(evaluation.total_positions, 2);
    }

    #[test]
    fn image_average_hash_decodes_artifact_bytes() {
        let mut image = image::ImageBuffer::<image::Luma<u8>, Vec<u8>>::new(8, 8);
        for (x, _y, pixel) in image.enumerate_pixels_mut() {
            *pixel = image::Luma([if x < 4 { 0 } else { 255 }]);
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("write png");

        assert_eq!(
            image_average_hash_hex(bytes.get_ref()).expect("image hash"),
            "0f0f0f0f0f0f0f0f"
        );
        assert!(image_average_hash_hex(b"not an image").is_err());
    }
}
