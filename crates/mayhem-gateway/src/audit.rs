use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ReputationEventKind;

pub const DEFAULT_CANARY_MATCH_MIN_BPS: u32 = 9_000;
pub const DEFAULT_CANARY_TEMPERATURE: f64 = 0.0;
pub const CANARY_VERIFICATION_TOKEN_FINGERPRINT: &str = "token_fingerprint";
pub const CANARY_VERIFICATION_CONTEXT_NEEDLE: &str = "context_needle";
pub const CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH: &str = "seed_perceptual_hash";
pub const CANARY_VERIFICATION_EMBEDDING_COSINE: &str = "embedding_cosine";
pub const CANARY_VERIFICATION_TRANSCRIPT_MATCH: &str = "transcript_match";
pub const CANARY_VERIFICATION_AUDIO_FINGERPRINT: &str = "audio_fingerprint";
pub const CANARY_VERIFICATION_ATTESTATION_OF_COMPUTE: &str = "attestation_of_compute";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanaryProbeSpec {
    pub model: String,
    pub canary_set: String,
    pub prompt_id: String,
    pub prompt: String,
    pub seed: i64,
    pub max_tokens: u32,
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
        json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": self.prompt,
                }
            ],
            "temperature": DEFAULT_CANARY_TEMPERATURE,
            "seed": self.seed,
            "max_tokens": self.max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
        })
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
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            if pending_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push(ch);
            pending_space = false;
        } else if ch.is_whitespace() || ch.is_ascii_punctuation() {
            pending_space = true;
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
    wav_audio_feature_fingerprint(bytes).unwrap_or_else(|| blake3::hash(bytes).to_hex().to_string())
}

fn wav_audio_feature_fingerprint(bytes: &[u8]) -> Option<String> {
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

    let bucket_count = 16_usize;
    let mut bucket_energy = vec![0_u64; bucket_count];
    let mut bucket_crossings = vec![0_u32; bucket_count];
    let mut bucket_samples = vec![0_u32; bucket_count];
    let mut previous = 0_i32;
    let mut has_previous = false;

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
        let mono = sum / i32::from(channels);
        let bucket = ((frame * bucket_count) / frame_count).min(bucket_count - 1);
        bucket_energy[bucket] =
            bucket_energy[bucket].saturating_add(u64::from(mono.unsigned_abs()));
        bucket_samples[bucket] = bucket_samples[bucket].saturating_add(1);
        if has_previous && ((previous < 0 && mono >= 0) || (previous >= 0 && mono < 0)) {
            bucket_crossings[bucket] = bucket_crossings[bucket].saturating_add(1);
        }
        previous = mono;
        has_previous = true;
    }

    let max_energy = bucket_energy
        .iter()
        .zip(bucket_samples.iter())
        .filter_map(|(energy, samples)| (*samples > 0).then_some(*energy / u64::from(*samples)))
        .max()
        .unwrap_or(1)
        .max(1);
    let duration_ms = ((frame_count as u128) * 1_000_u128 / u128::from(sample_rate)) as u64;
    let duration_bucket_ms = ((duration_ms + 50) / 100) * 100;
    let mut feature =
        format!("wav-pcm16-v1;sr={sample_rate};ch={channels};dur_ms={duration_bucket_ms};");
    for idx in 0..bucket_count {
        let avg_energy = if bucket_samples[idx] == 0 {
            0
        } else {
            bucket_energy[idx] / u64::from(bucket_samples[idx])
        };
        let energy_bucket = ((avg_energy * 7) / max_energy).min(7);
        let crossing_bucket = if bucket_samples[idx] == 0 {
            0
        } else {
            ((u64::from(bucket_crossings[idx]) * 7) / u64::from(bucket_samples[idx])).min(7)
        };
        feature.push_str(&format!("{energy_bucket:x}{crossing_bucket:x}"));
    }

    Some(blake3::hash(feature.as_bytes()).to_hex().to_string())
}

pub fn evaluate_catalog_canary_audio_fingerprint_probe(
    spec: &CanaryProbeSpec,
    expected_fingerprints_by_prompt: &BTreeMap<String, String>,
    observed_fingerprints_by_prompt: &BTreeMap<String, String>,
) -> CanaryProbeEvaluation {
    let mut matched_positions = 0_u32;
    let mut total_positions = 0_u32;
    let mut expected_fingerprints = Vec::with_capacity(expected_fingerprints_by_prompt.len());
    let mut observed_fingerprints = Vec::with_capacity(expected_fingerprints_by_prompt.len());

    for (prompt_id, expected) in expected_fingerprints_by_prompt {
        let observed = observed_fingerprints_by_prompt
            .get(prompt_id)
            .map(String::as_str)
            .unwrap_or_default();
        total_positions = total_positions.saturating_add(1);
        matched_positions = matched_positions.saturating_add(u32::from(
            !expected.is_empty() && expected.eq_ignore_ascii_case(observed),
        ));
        expected_fingerprints.push((prompt_id.as_str(), expected.to_ascii_lowercase()));
        observed_fingerprints.push((prompt_id.as_str(), observed.to_ascii_lowercase()));
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
        verification_method: CANARY_VERIFICATION_AUDIO_FINGERPRINT.to_owned(),
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
        }
    }

    #[test]
    fn canary_probe_request_is_regular_paid_openai_session_shape() {
        let body = spec().openai_chat_body();
        assert_eq!(body["model"], "mayhem/dev-chat-tools");
        assert_eq!(body["temperature"], 0.0);
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
