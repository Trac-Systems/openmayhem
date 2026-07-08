use mayhem_proto::{decimal_u128, MoneyAu};
use serde::{Deserialize, Serialize};

pub const DEFAULT_REPUTATION_HALF_LIFE_SECONDS: u64 = 14 * 24 * 60 * 60;
pub const DEFAULT_REPUTATION_KAPPA: f64 = 25.0;
pub const DEFAULT_PROBATION_SUCCESSFUL_SESSIONS: u64 = 50;
pub const DEFAULT_PROBATION_SECONDS: u64 = 7 * 24 * 60 * 60;
pub const DEFAULT_PROBATION_WEIGHT_BPS: u32 = 5_000;
pub const DEFAULT_PROBATION_MAX_CONCURRENT_SESSIONS_PER_USER: u32 = 2;
pub const DEFAULT_PROBATION_PRICE_MAX_BPS: u32 = 10_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReputationEvent {
    pub provider: String,
    pub event_id: String,
    pub epoch: u64,
    pub at_seconds: u64,
    pub evidence_hash: String,
    pub kind: ReputationEventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReputationEventKind {
    SessionOk {
        #[serde(with = "decimal_u128")]
        paid_au: MoneyAu,
    },
    SessionPartial {
        #[serde(with = "decimal_u128")]
        paid_au: MoneyAu,
    },
    SessionFail {
        #[serde(with = "decimal_u128")]
        max_spend_au: MoneyAu,
    },
    ProbeOk,
    ProbeFail,
    UptimeTick,
    Underdelivery,
    DisputeLost,
    ProvenanceViolation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProbationCaps {
    pub max_concurrent_sessions_per_user: u32,
    pub price_max_bps: u32,
    pub weight_bps: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProbationPolicy {
    pub required_successful_sessions: u64,
    pub required_seconds: u64,
    pub caps: ProbationCaps,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderProbation {
    pub active: bool,
    pub since_seconds: u64,
    pub successful_sessions: u64,
    pub required_successful_sessions: u64,
    pub required_seconds: u64,
    pub caps: ProbationCaps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReputationSnapshot {
    pub provider: String,
    pub r: f64,
    pub raw: f64,
    pub r_bps: u32,
    pub raw_milli: i64,
    pub events_head: String,
    pub epoch: u64,
    pub folded_at_seconds: u64,
    pub probation: ProviderProbation,
    pub provenance_violation: bool,
}

impl Default for ProbationCaps {
    fn default() -> Self {
        Self {
            max_concurrent_sessions_per_user: DEFAULT_PROBATION_MAX_CONCURRENT_SESSIONS_PER_USER,
            price_max_bps: DEFAULT_PROBATION_PRICE_MAX_BPS,
            weight_bps: DEFAULT_PROBATION_WEIGHT_BPS,
        }
    }
}

impl Default for ProbationPolicy {
    fn default() -> Self {
        Self {
            required_successful_sessions: DEFAULT_PROBATION_SUCCESSFUL_SESSIONS,
            required_seconds: DEFAULT_PROBATION_SECONDS,
            caps: ProbationCaps::default(),
        }
    }
}

impl ProviderProbation {
    pub fn from_policy(
        since_seconds: u64,
        successful_sessions: u64,
        at_seconds: u64,
        policy: &ProbationPolicy,
    ) -> Self {
        let enough_sessions = successful_sessions >= policy.required_successful_sessions;
        let old_enough = at_seconds.saturating_sub(since_seconds) >= policy.required_seconds;
        Self {
            active: !(enough_sessions && old_enough),
            since_seconds,
            successful_sessions,
            required_successful_sessions: policy.required_successful_sessions,
            required_seconds: policy.required_seconds,
            caps: policy.caps.clone(),
        }
    }

    pub fn weight_multiplier(&self) -> f64 {
        if self.active {
            f64::from(self.caps.weight_bps) / 10_000.0
        } else {
            1.0
        }
    }
}

impl ReputationEventKind {
    pub fn raw_score_and_weight(&self) -> Option<(f64, f64)> {
        match self {
            Self::SessionOk { paid_au } => Some((1.0, paid_weight(*paid_au))),
            Self::SessionPartial { paid_au } => Some((0.25, paid_weight(*paid_au))),
            Self::SessionFail { max_spend_au } => Some((-4.0, paid_weight(*max_spend_au))),
            Self::ProbeOk => Some((0.5, 1.0)),
            Self::ProbeFail => Some((-6.0, 1.0)),
            Self::UptimeTick => Some((0.1, 1.0)),
            Self::Underdelivery => Some((-2.0, 1.0)),
            Self::DisputeLost => Some((-20.0, 1.0)),
            Self::ProvenanceViolation => None,
        }
    }

    pub fn is_successful_paid_session(&self) -> bool {
        matches!(self, Self::SessionOk { .. })
    }

    pub fn is_provenance_violation(&self) -> bool {
        matches!(self, Self::ProvenanceViolation)
    }
}

pub fn fold_reputation(
    provider: impl Into<String>,
    events: &[ReputationEvent],
    at_seconds: u64,
    epoch: u64,
    probation_since_seconds: u64,
    initial_successful_sessions: u64,
    policy: &ProbationPolicy,
) -> Result<ReputationSnapshot, serde_json::Error> {
    let provider = provider.into();
    let provider_events = events
        .iter()
        .filter(|event| event.provider == provider)
        .cloned()
        .collect::<Vec<_>>();
    let raw = provider_events
        .iter()
        .filter_map(|event| {
            let (score, weight) = event.kind.raw_score_and_weight()?;
            let age = at_seconds.saturating_sub(event.at_seconds) as f64;
            let decay = 2.0_f64.powf(-(age / DEFAULT_REPUTATION_HALF_LIFE_SECONDS as f64));
            Some(score * weight * decay)
        })
        .sum::<f64>();
    let r = logistic_reputation(raw);
    let successful_sessions = initial_successful_sessions
        + provider_events
            .iter()
            .filter(|event| event.kind.is_successful_paid_session())
            .count() as u64;
    let probation = ProviderProbation::from_policy(
        probation_since_seconds,
        successful_sessions,
        at_seconds,
        policy,
    );
    let provenance_violation = provider_events
        .iter()
        .any(|event| event.kind.is_provenance_violation());

    Ok(ReputationSnapshot {
        provider,
        r,
        raw,
        r_bps: (r * 10_000.0).round().clamp(0.0, 10_000.0) as u32,
        raw_milli: (raw * 1000.0).round() as i64,
        events_head: reputation_events_head(&provider_events)?,
        epoch,
        folded_at_seconds: at_seconds,
        probation,
        provenance_violation,
    })
}

pub fn reputation_events_head(events: &[ReputationEvent]) -> Result<String, serde_json::Error> {
    let mut hasher = blake3::Hasher::new();
    for event in events {
        hasher.update(&serde_json::to_vec(event)?);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn logistic_reputation(raw: f64) -> f64 {
    1.0 / (1.0 + (-raw / DEFAULT_REPUTATION_KAPPA).exp())
}

fn paid_weight(au: MoneyAu) -> f64 {
    (1.0 + au as f64).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "aa";

    fn event(event_id: &str, at_seconds: u64, kind: ReputationEventKind) -> ReputationEvent {
        ReputationEvent {
            provider: PROVIDER.to_owned(),
            event_id: event_id.to_owned(),
            epoch: 1,
            at_seconds,
            evidence_hash: "11".repeat(32),
            kind,
        }
    }

    #[test]
    fn reputation_fold_matches_normative_formula_and_is_deterministic() {
        let events = vec![
            event("ok-1", 0, ReputationEventKind::SessionOk { paid_au: 9_999 }),
            event("tick-1", 0, ReputationEventKind::UptimeTick),
        ];

        let first =
            fold_reputation(PROVIDER, &events, 0, 1, 0, 0, &ProbationPolicy::default()).unwrap();
        let second =
            fold_reputation(PROVIDER, &events, 0, 1, 0, 0, &ProbationPolicy::default()).unwrap();

        let expected_raw = 1.0 * 4.0 + 0.1;
        assert!((first.raw - expected_raw).abs() < 1e-12);
        assert!((first.r - logistic_reputation(expected_raw)).abs() < 1e-12);
        assert_eq!(first, second);
        assert_eq!(first.r_bps, 5409);
        assert_eq!(first.probation.successful_sessions, 1);
        assert!(first.probation.active);
    }

    #[test]
    fn reputation_decay_uses_fourteen_day_half_life() {
        let events = vec![event(
            "ok-1",
            0,
            ReputationEventKind::SessionOk { paid_au: 9_999 },
        )];

        let folded = fold_reputation(
            PROVIDER,
            &events,
            DEFAULT_REPUTATION_HALF_LIFE_SECONDS,
            2,
            0,
            0,
            &ProbationPolicy::default(),
        )
        .unwrap();

        assert!((folded.raw - 2.0).abs() < 1e-12);
    }

    #[test]
    fn reputation_event_money_serializes_as_decimal_strings() {
        let event = event(
            "big-ok",
            0,
            ReputationEventKind::SessionOk {
                paid_au: 100_000_000_000_000_000_000,
            },
        );
        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(encoded["kind"]["paid_au"], "100000000000000000000");
        let decoded: ReputationEvent = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn underdelivery_is_a_soft_negative_reputation_event() {
        let folded = fold_reputation(
            PROVIDER,
            &[event("under-1", 0, ReputationEventKind::Underdelivery)],
            0,
            1,
            0,
            0,
            &ProbationPolicy::default(),
        )
        .unwrap();

        assert_eq!(folded.raw_milli, -2_000);
        assert_eq!(folded.r_bps, 4800);
        assert!(!folded.provenance_violation);
    }

    #[test]
    fn probation_requires_successful_sessions_and_age() {
        let policy = ProbationPolicy::default();
        let too_few_sessions = ProviderProbation::from_policy(
            0,
            DEFAULT_PROBATION_SUCCESSFUL_SESSIONS - 1,
            DEFAULT_PROBATION_SECONDS + 1,
            &policy,
        );
        assert!(too_few_sessions.active);

        let too_young = ProviderProbation::from_policy(
            0,
            DEFAULT_PROBATION_SUCCESSFUL_SESSIONS,
            DEFAULT_PROBATION_SECONDS - 1,
            &policy,
        );
        assert!(too_young.active);

        let cleared = ProviderProbation::from_policy(
            0,
            DEFAULT_PROBATION_SUCCESSFUL_SESSIONS,
            DEFAULT_PROBATION_SECONDS,
            &policy,
        );
        assert!(!cleared.active);
        assert_eq!(cleared.weight_multiplier(), 1.0);
        assert_eq!(too_few_sessions.weight_multiplier(), 0.5);
    }
}
