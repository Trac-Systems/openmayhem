use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ReputationEventKind;

pub const DEFAULT_CANARY_MATCH_MIN_BPS: u32 = 9_000;
pub const DEFAULT_CANARY_TEMPERATURE: f64 = 0.0;
pub const CANARY_VERIFICATION_TOKEN_FINGERPRINT: &str = "token_fingerprint";
pub const CANARY_VERIFICATION_CONTEXT_NEEDLE: &str = "context_needle";
pub const CANARY_VERIFICATION_SEED_PERCEPTUAL_HASH: &str = "seed_perceptual_hash";
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
}
