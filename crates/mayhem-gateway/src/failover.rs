use std::collections::{BTreeMap, BTreeSet};

use mayhem_proto::{CheckpointPolicy, ReceiptUsage};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{text_generation_rate_map, usage_map_mu, ProviderKey, RateMapEntry};

pub const DEFAULT_OPEN_TIMEOUT_MILLIS: u64 = 3_000;
pub const DEFAULT_MAX_OPEN_ATTEMPTS: u8 = 4;
pub const DEFAULT_PROVIDER_COOLOFF_MILLIS: u64 = 30_000;
pub const DEFAULT_STALL_TIMEOUT_MILLIS: u64 = 15_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FailoverPolicy {
    pub open_timeout_millis: u64,
    pub max_open_attempts: u8,
    pub provider_cooloff_millis: u64,
    pub stall_timeout_millis: u64,
    pub checkpoint_every: CheckpointPolicy,
    pub hedge_enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionPriceMu {
    pub rate_map: Vec<RateMapEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenAttempt {
    pub provider: ProviderKey,
    pub attempt: u8,
    pub started_at_millis: u64,
    pub deadline_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OpenRetryDecision {
    Retry {
        next_attempt: u8,
        cooled_until_millis: u64,
    },
    Exhausted {
        cooled_until_millis: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReceiptCheckpoint {
    pub seq: u64,
    pub final_receipt: bool,
    pub usage: ReceiptUsage,
    pub mu_owed_cum: u64,
    pub uncheckpointed_usage: ReceiptUsage,
    pub reason: String,
    pub ts_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RedispatchMode {
    FullMessageHistoryClientSide,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RedispatchPlan {
    pub stalled_provider: ProviderKey,
    pub cooled_until_millis: u64,
    pub partial_receipt: ReceiptCheckpoint,
    pub mode: RedispatchMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HedgePlan {
    pub probes: Vec<ProviderKey>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HedgeOutcome {
    pub winner: ProviderKey,
    pub losers_to_close: Vec<ProviderKey>,
    pub spend_allowed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HedgeRace {
    probes: Vec<ProviderKey>,
    winner: Option<ProviderKey>,
    losers_open: BTreeSet<ProviderKey>,
    spend_started: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionFailoverState {
    policy: FailoverPolicy,
    price: SessionPriceMu,
    input_tokens: u64,
    delivered_output_tokens: u64,
    last_receipted_usage: ReceiptUsage,
    last_receipted_mu: u64,
    next_receipt_seq: u64,
    open_attempts: u8,
    cooloffs: BTreeMap<ProviderKey, u64>,
    active_provider: Option<ProviderKey>,
    last_delta_at_millis: Option<u64>,
    last_receipt_at_millis: u64,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum FailoverError {
    #[error("open attempts exhausted")]
    OpenAttemptsExhausted,
    #[error("provider is in cooloff until {until_millis}ms")]
    ProviderInCooloff { until_millis: u64 },
    #[error("session has no active provider")]
    NoActiveProvider,
    #[error("frame came from a provider that is not active for this session")]
    NonActiveProvider,
    #[error("unexpected receipt sequence {actual}; expected {expected}")]
    UnexpectedReceiptSeq { expected: u64, actual: u64 },
    #[error("receipt cumulative usage regressed")]
    ReceiptUsageRegression,
    #[error("receipt cumulative amount regressed")]
    ReceiptMuRegression,
    #[error("unknown hedge provider")]
    UnknownHedgeProvider,
    #[error("hedge winner already chosen")]
    HedgeWinnerAlreadyChosen,
    #[error("hedge spend cannot start until {losers_remaining} loser probe(s) are closed")]
    HedgeSpendBeforeLosersClosed { losers_remaining: usize },
    #[error("hedge spend can only start on the winning provider")]
    HedgeProviderNotWinner,
}

impl Default for FailoverPolicy {
    fn default() -> Self {
        Self {
            open_timeout_millis: DEFAULT_OPEN_TIMEOUT_MILLIS,
            max_open_attempts: DEFAULT_MAX_OPEN_ATTEMPTS,
            provider_cooloff_millis: DEFAULT_PROVIDER_COOLOFF_MILLIS,
            stall_timeout_millis: DEFAULT_STALL_TIMEOUT_MILLIS,
            checkpoint_every: CheckpointPolicy {
                tokens: 8192,
                ms: 30_000,
            },
            hedge_enabled: true,
        }
    }
}

impl SessionPriceMu {
    pub fn text(in_per_1k_mu: u64, out_per_1k_mu: u64) -> Self {
        Self {
            rate_map: text_generation_rate_map(in_per_1k_mu, out_per_1k_mu),
        }
    }

    pub fn mu_for_usage(&self, usage: &ReceiptUsage) -> u64 {
        usage_map_mu(&self.rate_map, usage)
    }
}

impl SessionFailoverState {
    pub fn new(
        policy: FailoverPolicy,
        price: SessionPriceMu,
        input_tokens: u64,
        started_at_millis: u64,
    ) -> Self {
        Self {
            policy,
            price,
            input_tokens,
            delivered_output_tokens: 0,
            last_receipted_usage: ReceiptUsage::default(),
            last_receipted_mu: 0,
            next_receipt_seq: 1,
            open_attempts: 0,
            cooloffs: BTreeMap::new(),
            active_provider: None,
            last_delta_at_millis: None,
            last_receipt_at_millis: started_at_millis,
        }
    }

    pub fn policy(&self) -> &FailoverPolicy {
        &self.policy
    }

    pub fn current_usage(&self) -> ReceiptUsage {
        ReceiptUsage::text(self.input_tokens, self.delivered_output_tokens)
    }

    pub fn open_attempts(&self) -> u8 {
        self.open_attempts
    }

    pub fn cooloff_until(&self, provider: &ProviderKey) -> Option<u64> {
        self.cooloffs.get(provider).copied()
    }

    pub fn provider_in_cooloff(&self, provider: &ProviderKey, now_millis: u64) -> bool {
        self.cooloff_until(provider)
            .is_some_and(|until| until > now_millis)
    }

    pub fn available_candidates(
        &self,
        candidates: &[ProviderKey],
        now_millis: u64,
    ) -> Vec<ProviderKey> {
        candidates
            .iter()
            .filter(|candidate| !self.provider_in_cooloff(candidate, now_millis))
            .cloned()
            .collect()
    }

    pub fn start_open_attempt(
        &mut self,
        provider: ProviderKey,
        now_millis: u64,
    ) -> Result<OpenAttempt, FailoverError> {
        if self.open_attempts >= self.policy.max_open_attempts {
            return Err(FailoverError::OpenAttemptsExhausted);
        }
        if let Some(until_millis) = self
            .cooloff_until(&provider)
            .filter(|until_millis| *until_millis > now_millis)
        {
            return Err(FailoverError::ProviderInCooloff { until_millis });
        }

        self.open_attempts = self.open_attempts.saturating_add(1);
        self.active_provider = Some(provider.clone());
        Ok(OpenAttempt {
            provider,
            attempt: self.open_attempts,
            started_at_millis: now_millis,
            deadline_at_millis: now_millis.saturating_add(self.policy.open_timeout_millis),
        })
    }

    pub fn open_attempt_timed_out(&self, attempt: &OpenAttempt, now_millis: u64) -> bool {
        now_millis >= attempt.deadline_at_millis
    }

    pub fn record_open_timeout(
        &mut self,
        provider: &ProviderKey,
        now_millis: u64,
    ) -> OpenRetryDecision {
        let cooled_until_millis = self.cool_provider(provider, now_millis);
        if self.active_provider.as_ref() == Some(provider) {
            self.active_provider = None;
        }
        if self.open_attempts < self.policy.max_open_attempts {
            OpenRetryDecision::Retry {
                next_attempt: self.open_attempts.saturating_add(1),
                cooled_until_millis,
            }
        } else {
            OpenRetryDecision::Exhausted {
                cooled_until_millis,
            }
        }
    }

    pub fn record_open_success(&mut self, provider: ProviderKey, now_millis: u64) {
        self.active_provider = Some(provider);
        self.last_delta_at_millis = Some(now_millis);
    }

    pub fn record_delta(
        &mut self,
        provider: &ProviderKey,
        output_tokens: u64,
        now_millis: u64,
    ) -> Result<Option<ReceiptCheckpoint>, FailoverError> {
        self.require_active_provider(provider)?;
        self.delivered_output_tokens = self.delivered_output_tokens.saturating_add(output_tokens);
        self.last_delta_at_millis = Some(now_millis);
        Ok(self
            .checkpoint_due(now_millis)
            .then(|| self.receipt_checkpoint(false, "checkpoint", now_millis)))
    }

    pub fn checkpoint_due(&self, now_millis: u64) -> bool {
        let current = self.current_usage();
        let output_delta = current
            .output_tokens()
            .saturating_sub(self.last_receipted_usage.output_tokens());
        let input_delta = current
            .input_tokens()
            .saturating_sub(self.last_receipted_usage.input_tokens());
        if output_delta == 0 && input_delta == 0 {
            return false;
        }

        let output_tokens_due = output_delta > 0
            && (self.policy.checkpoint_every.tokens == 0
                || output_delta >= self.policy.checkpoint_every.tokens);
        let time_due = self.policy.checkpoint_every.ms == 0
            || now_millis.saturating_sub(self.last_receipt_at_millis)
                >= self.policy.checkpoint_every.ms;
        output_tokens_due || time_due
    }

    pub fn receipt_checkpoint(
        &self,
        final_receipt: bool,
        reason: impl Into<String>,
        now_millis: u64,
    ) -> ReceiptCheckpoint {
        let usage = self.current_usage();
        ReceiptCheckpoint {
            seq: self.next_receipt_seq,
            final_receipt,
            mu_owed_cum: self.price.mu_for_usage(&usage),
            uncheckpointed_usage: usage_delta(&self.last_receipted_usage, &usage),
            usage,
            reason: reason.into(),
            ts_millis: now_millis,
        }
    }

    pub fn record_receipt_cosigned(
        &mut self,
        checkpoint: &ReceiptCheckpoint,
        now_millis: u64,
    ) -> Result<(), FailoverError> {
        if checkpoint.seq != self.next_receipt_seq {
            return Err(FailoverError::UnexpectedReceiptSeq {
                expected: self.next_receipt_seq,
                actual: checkpoint.seq,
            });
        }
        if !checkpoint
            .usage
            .is_monotonic_from(&self.last_receipted_usage)
        {
            return Err(FailoverError::ReceiptUsageRegression);
        }
        if checkpoint.mu_owed_cum < self.last_receipted_mu {
            return Err(FailoverError::ReceiptMuRegression);
        }

        self.last_receipted_usage = checkpoint.usage.clone();
        self.last_receipted_mu = checkpoint.mu_owed_cum;
        self.last_receipt_at_millis = now_millis;
        self.next_receipt_seq = self.next_receipt_seq.saturating_add(1);
        Ok(())
    }

    pub fn midstream_stalled(&self, now_millis: u64) -> bool {
        self.last_delta_at_millis.is_some_and(|last_delta| {
            now_millis.saturating_sub(last_delta) > self.policy.stall_timeout_millis
        })
    }

    pub fn record_midstream_stall(&mut self, now_millis: u64) -> Option<RedispatchPlan> {
        if !self.midstream_stalled(now_millis) {
            return None;
        }
        let stalled_provider = self.active_provider.clone()?;
        let cooled_until_millis = self.cool_provider(&stalled_provider, now_millis);
        self.active_provider = None;
        Some(RedispatchPlan {
            stalled_provider,
            cooled_until_millis,
            partial_receipt: self.receipt_checkpoint(false, "mid_stream_stall", now_millis),
            mode: RedispatchMode::FullMessageHistoryClientSide,
        })
    }

    pub fn hedge_plan(
        &self,
        candidates: &[ProviderKey],
        header_value: Option<&str>,
        now_millis: u64,
    ) -> Option<HedgePlan> {
        if !self.policy.hedge_enabled || !x_mayhem_hedge_requested(header_value) {
            return None;
        }
        let probes = self
            .available_candidates(candidates, now_millis)
            .into_iter()
            .take(2)
            .collect::<Vec<_>>();
        (probes.len() == 2).then_some(HedgePlan { probes })
    }

    fn require_active_provider(&self, provider: &ProviderKey) -> Result<(), FailoverError> {
        match self.active_provider.as_ref() {
            Some(active) if active == provider => Ok(()),
            Some(_) => Err(FailoverError::NonActiveProvider),
            None => Err(FailoverError::NoActiveProvider),
        }
    }

    fn cool_provider(&mut self, provider: &ProviderKey, now_millis: u64) -> u64 {
        let cooled_until_millis = now_millis.saturating_add(self.policy.provider_cooloff_millis);
        self.cooloffs.insert(provider.clone(), cooled_until_millis);
        cooled_until_millis
    }
}

impl HedgeRace {
    pub fn from_plan(plan: HedgePlan) -> Self {
        Self {
            probes: plan.probes,
            winner: None,
            losers_open: BTreeSet::new(),
            spend_started: false,
        }
    }

    pub fn record_first_response(
        &mut self,
        provider: &ProviderKey,
    ) -> Result<HedgeOutcome, FailoverError> {
        if self.winner.is_some() {
            return Err(FailoverError::HedgeWinnerAlreadyChosen);
        }
        if !self.probes.iter().any(|probe| probe == provider) {
            return Err(FailoverError::UnknownHedgeProvider);
        }

        self.winner = Some(provider.clone());
        self.losers_open = self
            .probes
            .iter()
            .filter(|probe| *probe != provider)
            .cloned()
            .collect();
        Ok(HedgeOutcome {
            winner: provider.clone(),
            losers_to_close: self.losers_open.iter().cloned().collect(),
            spend_allowed: self.spend_allowed(),
        })
    }

    pub fn close_loser(&mut self, provider: &ProviderKey) -> Result<(), FailoverError> {
        if !self.losers_open.remove(provider) {
            return Err(FailoverError::UnknownHedgeProvider);
        }
        Ok(())
    }

    pub fn spend_allowed(&self) -> bool {
        self.winner.is_some() && self.losers_open.is_empty()
    }

    pub fn start_spend(&mut self, provider: &ProviderKey) -> Result<(), FailoverError> {
        if self.winner.as_ref() != Some(provider) {
            return Err(FailoverError::HedgeProviderNotWinner);
        }
        if !self.losers_open.is_empty() {
            return Err(FailoverError::HedgeSpendBeforeLosersClosed {
                losers_remaining: self.losers_open.len(),
            });
        }
        self.spend_started = true;
        Ok(())
    }

    pub fn spend_started(&self) -> bool {
        self.spend_started
    }
}

pub fn x_mayhem_hedge_requested(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == "1")
}

fn usage_delta(previous: &ReceiptUsage, current: &ReceiptUsage) -> ReceiptUsage {
    ReceiptUsage::saturating_delta(previous, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(idx: u8) -> ProviderKey {
        ProviderKey::new(
            format!("{idx:02x}").repeat(32),
            format!("{:02x}", idx.wrapping_add(80)).repeat(32),
            format!("{:02x}", idx.wrapping_add(160)).repeat(16),
        )
    }

    fn price() -> SessionPriceMu {
        SessionPriceMu::text(10, 100)
    }

    #[test]
    fn open_timeout_retries_four_attempts_and_sets_cooloff() {
        let mut state = SessionFailoverState::new(FailoverPolicy::default(), price(), 128, 1_000);
        let providers = (1..=5).map(provider).collect::<Vec<_>>();

        let first = state
            .start_open_attempt(providers[0].clone(), 1_000)
            .expect("first attempt");
        assert_eq!(first.attempt, 1);
        assert_eq!(first.deadline_at_millis, 4_000);
        assert!(!state.open_attempt_timed_out(&first, 3_999));
        assert!(state.open_attempt_timed_out(&first, 4_000));

        assert_eq!(
            state.record_open_timeout(&providers[0], 4_000),
            OpenRetryDecision::Retry {
                next_attempt: 2,
                cooled_until_millis: 34_000,
            }
        );
        assert!(state.provider_in_cooloff(&providers[0], 33_999));
        assert!(!state.provider_in_cooloff(&providers[0], 34_000));
        assert_eq!(
            state.available_candidates(&providers, 33_999),
            providers[1..].to_vec()
        );

        for (attempt_idx, provider) in providers[1..4].iter().enumerate() {
            let attempt = state
                .start_open_attempt(provider.clone(), 10_000 + attempt_idx as u64)
                .expect("retry attempt");
            assert_eq!(attempt.attempt, attempt_idx as u8 + 2);
            let decision = state.record_open_timeout(provider, attempt.deadline_at_millis);
            if attempt.attempt < DEFAULT_MAX_OPEN_ATTEMPTS {
                assert!(matches!(decision, OpenRetryDecision::Retry { .. }));
            } else {
                assert!(matches!(decision, OpenRetryDecision::Exhausted { .. }));
            }
        }

        assert_eq!(state.open_attempts(), DEFAULT_MAX_OPEN_ATTEMPTS);
        assert_eq!(
            state.start_open_attempt(providers[4].clone(), 20_000),
            Err(FailoverError::OpenAttemptsExhausted)
        );
    }

    #[test]
    fn midstream_stall_returns_partial_receipt_and_cumulative_redispatch_accounting() {
        let policy = FailoverPolicy {
            checkpoint_every: CheckpointPolicy {
                tokens: 8,
                ms: 30_000,
            },
            ..FailoverPolicy::default()
        };
        let mut state = SessionFailoverState::new(policy, price(), 1_000, 0);
        let first = provider(1);
        let second = provider(2);
        state.record_open_success(first.clone(), 0);

        let checkpoint = state
            .record_delta(&first, 8, 100)
            .expect("delta accepted")
            .expect("checkpoint due");
        assert_eq!(checkpoint.seq, 1);
        assert_eq!(checkpoint.usage, ReceiptUsage::text(1_000, 8));
        assert_eq!(checkpoint.uncheckpointed_usage.output_tokens(), 8);
        state
            .record_receipt_cosigned(&checkpoint, 101)
            .expect("checkpoint receipt accepted");

        assert!(state
            .record_delta(&first, 2, 200)
            .expect("delta accepted")
            .is_none());
        assert!(!state.midstream_stalled(15_200));
        assert!(state.midstream_stalled(15_201));

        let redispatch = state
            .record_midstream_stall(15_201)
            .expect("redispatch planned");
        assert_eq!(redispatch.stalled_provider, first);
        assert_eq!(
            redispatch.mode,
            RedispatchMode::FullMessageHistoryClientSide
        );
        assert_eq!(redispatch.partial_receipt.seq, 2);
        assert!(!redispatch.partial_receipt.final_receipt);
        assert_eq!(
            redispatch.partial_receipt.usage,
            ReceiptUsage::text(1_000, 10)
        );
        assert_eq!(
            redispatch.partial_receipt.uncheckpointed_usage,
            ReceiptUsage::text(0, 2)
        );
        assert!(
            redispatch
                .partial_receipt
                .uncheckpointed_usage
                .output_tokens()
                <= state.policy().checkpoint_every.tokens
        );
        state
            .record_receipt_cosigned(&redispatch.partial_receipt, 15_202)
            .expect("partial receipt accepted");

        state.record_open_success(second.clone(), 15_500);
        state
            .record_delta(&second, 5, 15_600)
            .expect("second provider delta accepted");
        let final_receipt = state.receipt_checkpoint(true, "final", 15_700);
        assert_eq!(final_receipt.usage, ReceiptUsage::text(1_000, 15));
        assert_eq!(
            final_receipt.mu_owed_cum,
            price().mu_for_usage(&final_receipt.usage)
        );

        let duplicated_prompt_charge = redispatch.partial_receipt.mu_owed_cum
            + price().mu_for_usage(&ReceiptUsage::text(1_000, 5));
        assert!(final_receipt.mu_owed_cum < duplicated_prompt_charge);
    }

    #[test]
    fn x_mayhem_hedge_probe_closes_loser_before_spend() {
        let state = SessionFailoverState::new(FailoverPolicy::default(), price(), 128, 0);
        let providers = vec![provider(1), provider(2), provider(3)];

        assert!(!x_mayhem_hedge_requested(None));
        assert!(!x_mayhem_hedge_requested(Some("0")));
        assert!(x_mayhem_hedge_requested(Some("1")));
        assert!(state.hedge_plan(&providers, None, 0).is_none());

        let plan = state
            .hedge_plan(&providers, Some("1"), 0)
            .expect("hedge plan");
        assert_eq!(plan.probes, providers[0..2].to_vec());

        let mut race = HedgeRace::from_plan(plan);
        let outcome = race
            .record_first_response(&providers[1])
            .expect("winner accepted");
        assert_eq!(outcome.winner, providers[1]);
        assert_eq!(outcome.losers_to_close, vec![providers[0].clone()]);
        assert!(!outcome.spend_allowed);
        assert_eq!(
            race.start_spend(&providers[1]),
            Err(FailoverError::HedgeSpendBeforeLosersClosed {
                losers_remaining: 1,
            })
        );

        race.close_loser(&providers[0]).expect("loser closed");
        race.start_spend(&providers[1]).expect("spend starts");
        assert!(race.spend_started());
        assert_eq!(
            race.start_spend(&providers[0]),
            Err(FailoverError::HedgeProviderNotWinner)
        );
    }
}
