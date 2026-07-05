use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ReputationEventKind;

pub const DEFAULT_CANARY_MATCH_MIN_BPS: u32 = 9_000;
pub const DEFAULT_CANARY_TEMPERATURE: f64 = 0.0;

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
}
