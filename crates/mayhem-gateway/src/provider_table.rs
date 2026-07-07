use std::collections::BTreeMap;

use mayhem_proto::ReceiptUsage;
use serde::{Deserialize, Serialize};

use crate::{text_usage_mu, HeartbeatCaps, ProviderHeartbeat, ProviderProbation, RateMapEntry};

pub const DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS: u64 = 10_000;
pub const DEFAULT_OBSERVATION_EWMA_ALPHA: f64 = 0.2;
pub const DEFAULT_ATTESTATION_HEAD_MAX_AGE_MILLIS: u64 = 24 * 60 * 60 * 1000;
pub const DEFAULT_SATURATION_CUTOFF: f64 = 0.85;
pub const DEFAULT_REPUTATION_ALPHA: f64 = 1.5;
pub const DEFAULT_SATURATION_BETA: f64 = 1.0;
pub const DEFAULT_PRICE_GAMMA: f64 = 0.7;
const P2C_REPUTATION_DECISION_DELTA: f64 = 0.05;
pub const DEFAULT_ERROR_CIRCUIT_BREAKER_MIN_SAMPLES: u64 = 3;
pub const DEFAULT_ERROR_CIRCUIT_BREAKER_EWMA_THRESHOLD: f64 = 0.8;
pub const DEFAULT_ERROR_CIRCUIT_BREAKER_CONSECUTIVE_FAILURES: u32 = 3;
pub const DEFAULT_ERROR_CIRCUIT_BREAKER_COOLOFF_MILLIS: u64 = 30_000;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ProviderKey {
    pub provider: String,
    pub enclave_id: String,
    pub room_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContractProviderSnapshot {
    pub provider: String,
    #[serde(default, alias = "status")]
    pub provider_status: Option<String>,
    pub enclave_id: String,
    pub model_id: String,
    pub room_id: String,
    pub consent_ver: u64,
    pub reputation: f64,
    pub price_ver: u64,
    pub rate_map: Vec<RateMapEntry>,
    pub ref_rate_map: Vec<RateMapEntry>,
    pub probation: Option<ProviderProbation>,
    pub caps: HeartbeatCaps,
    pub attestation_head: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderObservationSample {
    pub ttft_ms: u64,
    pub tok_s: Option<f64>,
    pub error: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderObservation {
    pub ewma_ttft_ms: Option<f64>,
    pub ewma_tok_s: Option<f64>,
    pub ewma_error_rate: f64,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_open_until_millis: Option<u64>,
    pub samples: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttestationHeadCacheEntry {
    pub provider: String,
    pub enclave_id: String,
    pub head: String,
    pub epoch: u64,
    pub observed_at_millis: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderTableEntry {
    pub key: ProviderKey,
    pub contract: ContractProviderSnapshot,
    pub heartbeat: Option<ProviderHeartbeat>,
    pub heartbeat_age_millis: Option<u64>,
    pub observed: ProviderObservation,
    pub attestation_head: Option<AttestationHeadCacheEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestRequirements {
    pub current_rules_ver: u64,
    pub min_reputation: f64,
    pub requires_tools: bool,
    pub requires_json: bool,
    pub requires_vision: bool,
    pub min_ctx: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub max_price_mu: Option<u64>,
    pub now_millis: u64,
    pub max_attestation_head_age_millis: u64,
    pub heartbeat_ttl_millis: u64,
    pub saturation_cutoff: f64,
    pub provider_user_active_sessions: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IneligibilityReason {
    ConsentVersion,
    Reputation,
    HeartbeatMissing,
    HeartbeatStale,
    Saturated,
    Capabilities,
    Price,
    ProbationConcurrentLimit,
    ProbationPriceCap,
    AttestationMissing,
    AttestationStale,
    CircuitOpen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionWeights {
    pub reputation_alpha: f64,
    pub saturation_beta: f64,
    pub price_gamma: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelectionCandidate {
    pub entry: ProviderTableEntry,
    pub estimated_price_mu: u64,
    pub effective_ttft_ms: f64,
    pub latency_factor: f64,
    pub price_norm: f64,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSelection {
    pub selected: SelectionCandidate,
    pub sampled: Vec<ProviderKey>,
}

pub trait BalancerRng {
    fn next_unit_f64(&mut self) -> f64;
}

#[derive(Clone, Debug)]
pub struct LcgBalancerRng {
    state: u64,
}

#[derive(Clone, Debug)]
struct LiveHeartbeat {
    heartbeat: ProviderHeartbeat,
    received_at_millis: u64,
}

#[derive(Clone, Debug)]
pub struct ProviderTable {
    contract: BTreeMap<ProviderKey, ContractProviderSnapshot>,
    heartbeats: BTreeMap<ProviderKey, LiveHeartbeat>,
    observations: BTreeMap<ProviderKey, ProviderObservation>,
    attestation_heads: BTreeMap<(String, String), AttestationHeadCacheEntry>,
    heartbeat_ttl_millis: u64,
    observation_alpha: f64,
}

impl ProviderKey {
    pub fn new(
        provider: impl Into<String>,
        enclave_id: impl Into<String>,
        room_id: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            enclave_id: enclave_id.into(),
            room_id: room_id.into(),
        }
    }

    pub fn from_contract(record: &ContractProviderSnapshot) -> Self {
        Self::new(
            record.provider.clone(),
            record.enclave_id.clone(),
            record.room_id.clone(),
        )
    }

    pub fn from_heartbeat(heartbeat: &ProviderHeartbeat) -> Self {
        Self::new(
            heartbeat.provider.clone(),
            heartbeat.enclave_id.clone(),
            heartbeat.room_id.clone(),
        )
    }
}

impl Default for ProviderTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for RequestRequirements {
    fn default() -> Self {
        Self {
            current_rules_ver: 1,
            min_reputation: 0.0,
            requires_tools: false,
            requires_json: false,
            requires_vision: false,
            min_ctx: 0,
            input_tokens: 0,
            output_tokens: 0,
            max_price_mu: None,
            now_millis: 0,
            max_attestation_head_age_millis: DEFAULT_ATTESTATION_HEAD_MAX_AGE_MILLIS,
            heartbeat_ttl_millis: DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS,
            saturation_cutoff: DEFAULT_SATURATION_CUTOFF,
            provider_user_active_sessions: BTreeMap::new(),
        }
    }
}

impl Default for SelectionWeights {
    fn default() -> Self {
        Self {
            reputation_alpha: DEFAULT_REPUTATION_ALPHA,
            saturation_beta: DEFAULT_SATURATION_BETA,
            price_gamma: DEFAULT_PRICE_GAMMA,
        }
    }
}

impl LcgBalancerRng {
    pub fn seeded(seed: u64) -> Self {
        Self { state: seed }
    }
}

impl BalancerRng for LcgBalancerRng {
    fn next_unit_f64(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let bits = self.state >> 11;
        (bits as f64) * (1.0 / ((1_u64 << 53) as f64))
    }
}

impl ProviderTable {
    pub fn new() -> Self {
        Self::with_config(
            DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS,
            DEFAULT_OBSERVATION_EWMA_ALPHA,
        )
    }

    pub fn with_config(heartbeat_ttl_millis: u64, observation_alpha: f64) -> Self {
        Self {
            contract: BTreeMap::new(),
            heartbeats: BTreeMap::new(),
            observations: BTreeMap::new(),
            attestation_heads: BTreeMap::new(),
            heartbeat_ttl_millis,
            observation_alpha: observation_alpha.clamp(0.0, 1.0),
        }
    }

    pub fn replace_contract_snapshot(
        &mut self,
        records: impl IntoIterator<Item = ContractProviderSnapshot>,
    ) {
        self.contract.clear();
        for record in records {
            self.upsert_contract(record);
        }
    }

    pub fn upsert_contract(&mut self, record: ContractProviderSnapshot) {
        if !provider_status_allows_routing(record.provider_status.as_deref()) {
            return;
        }
        if !canonical_room_id(&record.room_id) {
            return;
        }
        let key = ProviderKey::from_contract(&record);
        if let Some(head) = record.attestation_head.clone() {
            self.upsert_attestation_head(&record.provider, &record.enclave_id, head, 0, 0);
        }
        self.contract.insert(key, record);
    }

    pub fn upsert_heartbeat(&mut self, heartbeat: ProviderHeartbeat, received_at_millis: u64) {
        let key = ProviderKey::from_heartbeat(&heartbeat);
        self.upsert_attestation_head(
            &heartbeat.provider,
            &heartbeat.enclave_id,
            heartbeat.att.head.clone(),
            heartbeat.att.epoch,
            received_at_millis,
        );
        self.heartbeats.insert(
            key,
            LiveHeartbeat {
                heartbeat,
                received_at_millis,
            },
        );
    }

    pub fn record_observation(&mut self, key: &ProviderKey, sample: ProviderObservationSample) {
        self.record_observation_at(key, sample, 0);
    }

    pub fn record_observation_at(
        &mut self,
        key: &ProviderKey,
        sample: ProviderObservationSample,
        now_millis: u64,
    ) {
        let alpha = self.observation_alpha;
        let observed = self.observations.entry(key.clone()).or_default();
        observed.ewma_ttft_ms = Some(update_ewma(
            observed.ewma_ttft_ms,
            sample.ttft_ms as f64,
            alpha,
        ));
        if let Some(tok_s) = sample
            .tok_s
            .filter(|value| value.is_finite() && *value >= 0.0)
        {
            observed.ewma_tok_s = Some(update_ewma(observed.ewma_tok_s, tok_s, alpha));
        }
        observed.ewma_error_rate = update_ewma(
            if observed.samples == 0 {
                None
            } else {
                Some(observed.ewma_error_rate)
            },
            if sample.error { 1.0 } else { 0.0 },
            alpha,
        );
        if sample.error {
            observed.consecutive_failures = observed.consecutive_failures.saturating_add(1);
            if observed.consecutive_failures >= DEFAULT_ERROR_CIRCUIT_BREAKER_CONSECUTIVE_FAILURES
                || (observed.samples + 1 >= DEFAULT_ERROR_CIRCUIT_BREAKER_MIN_SAMPLES
                    && observed.ewma_error_rate >= DEFAULT_ERROR_CIRCUIT_BREAKER_EWMA_THRESHOLD)
            {
                observed.circuit_open_until_millis =
                    Some(now_millis.saturating_add(DEFAULT_ERROR_CIRCUIT_BREAKER_COOLOFF_MILLIS));
            }
        } else {
            observed.consecutive_failures = 0;
            observed.circuit_open_until_millis = None;
        }
        observed.samples += 1;
    }

    pub fn entries(&self, now_millis: u64) -> Vec<ProviderTableEntry> {
        self.contract
            .iter()
            .map(|(key, contract)| {
                let live = self.heartbeats.get(key);
                let heartbeat_age_millis =
                    live.map(|live| now_millis.saturating_sub(live.received_at_millis));
                let heartbeat = live
                    .filter(|live| {
                        now_millis.saturating_sub(live.received_at_millis)
                            <= self.heartbeat_ttl_millis
                    })
                    .map(|live| live.heartbeat.clone());
                ProviderTableEntry {
                    key: key.clone(),
                    contract: contract.clone(),
                    heartbeat,
                    heartbeat_age_millis,
                    observed: self.observations.get(key).cloned().unwrap_or_default(),
                    attestation_head: self
                        .attestation_head(&key.provider, &key.enclave_id)
                        .cloned(),
                }
            })
            .collect()
    }

    pub fn attestation_head(
        &self,
        provider: &str,
        enclave_id: &str,
    ) -> Option<&AttestationHeadCacheEntry> {
        self.attestation_heads
            .get(&(provider.to_owned(), enclave_id.to_owned()))
    }

    pub fn contract_len(&self) -> usize {
        self.contract.len()
    }

    pub fn heartbeat_len(&self) -> usize {
        self.heartbeats.len()
    }

    fn upsert_attestation_head(
        &mut self,
        provider: &str,
        enclave_id: &str,
        head: String,
        epoch: u64,
        observed_at_millis: u64,
    ) {
        let key = (provider.to_owned(), enclave_id.to_owned());
        let should_update = match self.attestation_heads.get(&key) {
            Some(existing) => epoch >= existing.epoch,
            None => true,
        };
        if should_update {
            self.attestation_heads.insert(
                key,
                AttestationHeadCacheEntry {
                    provider: provider.to_owned(),
                    enclave_id: enclave_id.to_owned(),
                    head,
                    epoch,
                    observed_at_millis,
                },
            );
        }
    }
}

pub fn evaluate_eligibility(
    entry: &ProviderTableEntry,
    request: &RequestRequirements,
) -> Result<u64, IneligibilityReason> {
    if entry.contract.consent_ver != request.current_rules_ver {
        return Err(IneligibilityReason::ConsentVersion);
    }
    if entry.contract.reputation < request.min_reputation {
        return Err(IneligibilityReason::Reputation);
    }
    if entry
        .observed
        .circuit_open_until_millis
        .is_some_and(|until| until > request.now_millis)
    {
        return Err(IneligibilityReason::CircuitOpen);
    }
    let heartbeat = entry
        .heartbeat
        .as_ref()
        .ok_or(IneligibilityReason::HeartbeatMissing)?;
    let heartbeat_age = entry
        .heartbeat_age_millis
        .ok_or(IneligibilityReason::HeartbeatMissing)?;
    if heartbeat_age >= request.heartbeat_ttl_millis {
        return Err(IneligibilityReason::HeartbeatStale);
    }
    if heartbeat.sat >= request.saturation_cutoff {
        return Err(IneligibilityReason::Saturated);
    }
    if request.requires_tools && !heartbeat.caps.tools
        || request.requires_json && !heartbeat.caps.json
        || request.requires_vision && !heartbeat.caps.vision
        || heartbeat.caps.ctx < request.min_ctx
    {
        return Err(IneligibilityReason::Capabilities);
    }
    let estimated_price_mu = estimate_request_price_mu(&entry.contract, request);
    if request
        .max_price_mu
        .is_some_and(|max_price_mu| estimated_price_mu > max_price_mu)
    {
        return Err(IneligibilityReason::Price);
    }
    if let Some(probation) = entry
        .contract
        .probation
        .as_ref()
        .filter(|probation| probation.active)
    {
        let active_sessions = request
            .provider_user_active_sessions
            .get(&entry.contract.provider)
            .copied()
            .unwrap_or_default();
        if active_sessions >= probation.caps.max_concurrent_sessions_per_user {
            return Err(IneligibilityReason::ProbationConcurrentLimit);
        }
        let reference_price_mu = estimate_reference_request_price_mu(&entry.contract, request);
        if probation_price_over_cap(
            estimated_price_mu,
            reference_price_mu,
            probation.caps.price_max_bps,
        ) {
            return Err(IneligibilityReason::ProbationPriceCap);
        }
    }
    let attestation = entry
        .attestation_head
        .as_ref()
        .ok_or(IneligibilityReason::AttestationMissing)?;
    let attestation_age = request
        .now_millis
        .saturating_sub(attestation.observed_at_millis);
    if attestation_age > request.max_attestation_head_age_millis {
        return Err(IneligibilityReason::AttestationStale);
    }
    Ok(estimated_price_mu)
}

pub fn eligible_candidates(
    entries: &[ProviderTableEntry],
    request: &RequestRequirements,
    weights: &SelectionWeights,
) -> Vec<SelectionCandidate> {
    let mut base = entries
        .iter()
        .filter_map(|entry| {
            let estimated_price_mu = evaluate_eligibility(entry, request).ok()?;
            Some((
                entry.clone(),
                estimated_price_mu,
                effective_ttft_ms(entry).max(1.0),
            ))
        })
        .collect::<Vec<_>>();
    if base.is_empty() {
        return Vec::new();
    }

    let median_ttft = median(base.iter().map(|(_, _, ttft)| *ttft).collect()).max(1.0);
    let median_price = median(
        base.iter()
            .map(|(_, price, _)| (*price).max(1) as f64)
            .collect(),
    )
    .max(1.0);

    base.drain(..)
        .map(|(entry, estimated_price_mu, effective_ttft_ms)| {
            let heartbeat = entry
                .heartbeat
                .as_ref()
                .expect("eligible candidates have live heartbeats");
            let reputation = entry.contract.reputation.clamp(0.0, 1.0);
            let available = (1.0 - heartbeat.sat).clamp(0.0, 1.0);
            let price_norm = ((estimated_price_mu.max(1) as f64) / median_price).max(f64::EPSILON);
            let latency_factor = (median_ttft / effective_ttft_ms).clamp(0.25, 4.0);
            let error_factor = (1.0 - entry.observed.ewma_error_rate).clamp(0.05, 1.0);
            let weight = reputation.powf(weights.reputation_alpha)
                * available.powf(weights.saturation_beta)
                * (1.0 / price_norm).powf(weights.price_gamma)
                * latency_factor
                * error_factor
                * probation_weight_multiplier(&entry);
            SelectionCandidate {
                entry,
                estimated_price_mu,
                effective_ttft_ms,
                latency_factor,
                price_norm,
                weight,
            }
        })
        .collect()
}

pub fn select_weighted_p2c(
    entries: &[ProviderTableEntry],
    request: &RequestRequirements,
    weights: &SelectionWeights,
    rng: &mut impl BalancerRng,
) -> Option<ProviderSelection> {
    let candidates = eligible_candidates(entries, request, weights);
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(ProviderSelection {
            selected: candidates[0].clone(),
            sampled: vec![candidates[0].entry.key.clone()],
        });
    }

    let first = weighted_sample_index(&candidates, None, rng)?;
    let second = weighted_sample_index(&candidates, Some(first), rng).unwrap_or(first);
    let selected = better_p2c_index(&candidates, first, second);
    Some(ProviderSelection {
        selected: candidates[selected].clone(),
        sampled: vec![
            candidates[first].entry.key.clone(),
            candidates[second].entry.key.clone(),
        ],
    })
}

pub fn estimate_request_price_mu(
    contract: &ContractProviderSnapshot,
    request: &RequestRequirements,
) -> u64 {
    let usage = ReceiptUsage::text(request.input_tokens, request.output_tokens);
    text_usage_mu(&contract.rate_map, &usage)
}

pub fn estimate_reference_request_price_mu(
    contract: &ContractProviderSnapshot,
    request: &RequestRequirements,
) -> u64 {
    let usage = ReceiptUsage::text(request.input_tokens, request.output_tokens);
    text_usage_mu(&contract.ref_rate_map, &usage)
}

fn probation_price_over_cap(
    estimated_price_mu: u64,
    reference_price_mu: u64,
    cap_bps: u32,
) -> bool {
    u128::from(estimated_price_mu) * 10_000 > u128::from(reference_price_mu) * u128::from(cap_bps)
}

fn update_ewma(current: Option<f64>, sample: f64, alpha: f64) -> f64 {
    match current {
        Some(current) => alpha * sample + (1.0 - alpha) * current,
        None => sample,
    }
}

fn effective_ttft_ms(entry: &ProviderTableEntry) -> f64 {
    entry
        .observed
        .ewma_ttft_ms
        .or_else(|| {
            entry
                .heartbeat
                .as_ref()
                .map(|heartbeat| heartbeat.perf.ttft_ms as f64)
        })
        .unwrap_or(1.0)
}

fn probation_weight_multiplier(entry: &ProviderTableEntry) -> f64 {
    entry
        .contract
        .probation
        .as_ref()
        .map(ProviderProbation::weight_multiplier)
        .unwrap_or(1.0)
}

fn canonical_room_id(value: &str) -> bool {
    value.len() == 32 && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn provider_status_allows_routing(value: Option<&str>) -> bool {
    value
        .map(|status| status.eq_ignore_ascii_case("active"))
        .unwrap_or(false)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|left, right| left.total_cmp(right));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn weighted_sample_index(
    candidates: &[SelectionCandidate],
    excluded: Option<usize>,
    rng: &mut impl BalancerRng,
) -> Option<usize> {
    let total = candidates
        .iter()
        .enumerate()
        .filter(|(idx, _)| Some(*idx) != excluded)
        .map(|(_, candidate)| candidate.weight.max(0.0))
        .sum::<f64>();
    if total <= f64::EPSILON {
        return candidates
            .iter()
            .enumerate()
            .find(|(idx, _)| Some(*idx) != excluded)
            .map(|(idx, _)| idx);
    }
    let unit = rng.next_unit_f64();
    let unit = if unit.is_finite() {
        unit.clamp(0.0, 1.0 - f64::EPSILON)
    } else {
        0.0
    };
    let mut target = unit * total;
    let mut fallback = None;
    for (idx, candidate) in candidates.iter().enumerate() {
        if Some(idx) == excluded {
            continue;
        }
        fallback = Some(idx);
        let weight = candidate.weight.max(0.0);
        if target < weight {
            return Some(idx);
        }
        target -= weight;
    }
    fallback
}

fn better_p2c_index(candidates: &[SelectionCandidate], left: usize, right: usize) -> usize {
    let left_reputation = candidates[left].entry.contract.reputation.clamp(0.0, 1.0)
        * probation_weight_multiplier(&candidates[left].entry);
    let right_reputation = candidates[right].entry.contract.reputation.clamp(0.0, 1.0)
        * probation_weight_multiplier(&candidates[right].entry);
    if (left_reputation - right_reputation).abs() >= P2C_REPUTATION_DECISION_DELTA {
        if left_reputation > right_reputation {
            return left;
        }
        return right;
    }
    let left_sat = candidates[left]
        .entry
        .heartbeat
        .as_ref()
        .map(|heartbeat| heartbeat.sat)
        .unwrap_or(1.0);
    let right_sat = candidates[right]
        .entry
        .heartbeat
        .as_ref()
        .map(|heartbeat| heartbeat.sat)
        .unwrap_or(1.0);
    match left_sat.total_cmp(&right_sat) {
        std::cmp::Ordering::Less => left,
        std::cmp::Ordering::Greater => right,
        std::cmp::Ordering::Equal => {
            if candidates[left].effective_ttft_ms <= candidates[right].effective_ttft_ms {
                left
            } else {
                right
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_generation_rate_map;
    use crate::{
        HeartbeatAttestation, HeartbeatPerf, HeartbeatQueue, HeartbeatSlots, ProbationCaps,
    };

    fn key() -> ProviderKey {
        ProviderKey::new("aa".repeat(32), "bb".repeat(32), "cc".repeat(16))
    }

    fn key_for(idx: u8) -> ProviderKey {
        ProviderKey::new(
            format!("{idx:02x}").repeat(32),
            format!("{:02x}", idx.wrapping_add(80)).repeat(32),
            format!("{:02x}", idx.wrapping_add(160)).repeat(16),
        )
    }

    fn caps() -> HeartbeatCaps {
        HeartbeatCaps {
            tools: true,
            json: true,
            ctx: 8192,
            vision: false,
        }
    }

    fn contract_record_for(idx: u8) -> ContractProviderSnapshot {
        let key = key_for(idx);
        ContractProviderSnapshot {
            provider: key.provider,
            provider_status: Some("active".to_owned()),
            enclave_id: key.enclave_id,
            model_id: "model/test@4bit".to_owned(),
            room_id: key.room_id,
            consent_ver: 3,
            reputation: 0.8,
            price_ver: 5,
            rate_map: text_generation_rate_map(20, 60),
            ref_rate_map: text_generation_rate_map(20, 60),
            probation: None,
            caps: caps(),
            attestation_head: None,
        }
    }

    fn contract_record() -> ContractProviderSnapshot {
        let key = key();
        ContractProviderSnapshot {
            provider: key.provider,
            provider_status: Some("active".to_owned()),
            enclave_id: key.enclave_id,
            model_id: "model/test@4bit".to_owned(),
            room_id: key.room_id,
            consent_ver: 3,
            reputation: 0.72,
            price_ver: 5,
            rate_map: text_generation_rate_map(20, 60),
            ref_rate_map: text_generation_rate_map(20, 60),
            probation: None,
            caps: caps(),
            attestation_head: None,
        }
    }

    fn heartbeat_for(
        idx: u8,
        ts: u64,
        sat: f64,
        ttft_ms: u64,
        epoch: u64,
        head: &str,
    ) -> ProviderHeartbeat {
        let key = key_for(idx);
        ProviderHeartbeat {
            t: "hb".to_owned(),
            v: crate::HEARTBEAT_SCHEMA_VERSION,
            contract_version: mayhem_proto::CONTRACT_VERSION,
            provider: key.provider,
            enclave_id: key.enclave_id,
            model_id: "model/test@4bit".to_owned(),
            room_id: key.room_id,
            sat,
            slots: HeartbeatSlots { active: 1, max: 8 },
            q: HeartbeatQueue {
                depth: 0,
                est_wait_ms: 0,
            },
            perf: HeartbeatPerf {
                tok_s: Some(50.0),
                ttft_ms,
            },
            price_ver: 5,
            caps: caps(),
            att: HeartbeatAttestation {
                epoch,
                head: head.repeat(32),
            },
            ts,
            nonce: format!("{idx:02x}").repeat(32),
            sig: "ee".repeat(64),
        }
    }

    fn heartbeat(ts: u64, epoch: u64, head: &str) -> ProviderHeartbeat {
        let key = key();
        ProviderHeartbeat {
            t: "hb".to_owned(),
            v: crate::HEARTBEAT_SCHEMA_VERSION,
            contract_version: mayhem_proto::CONTRACT_VERSION,
            provider: key.provider,
            enclave_id: key.enclave_id,
            model_id: "model/test@4bit".to_owned(),
            room_id: key.room_id,
            sat: 0.25,
            slots: HeartbeatSlots { active: 2, max: 8 },
            q: HeartbeatQueue {
                depth: 1,
                est_wait_ms: 50,
            },
            perf: HeartbeatPerf {
                tok_s: Some(48.0),
                ttft_ms: 140,
            },
            price_ver: 5,
            caps: caps(),
            att: HeartbeatAttestation {
                epoch,
                head: head.repeat(32),
            },
            ts,
            nonce: "dd".repeat(32),
            sig: "ee".repeat(64),
        }
    }

    fn entry_for(idx: u8, now: u64, sat: f64, ttft_ms: u64) -> ProviderTableEntry {
        let mut table = ProviderTable::new();
        table.upsert_contract(contract_record_for(idx));
        table.upsert_heartbeat(heartbeat_for(idx, now, sat, ttft_ms, 9, "44"), now);
        table.entries(now + 1).pop().expect("provider entry")
    }

    fn eligible_request(now: u64) -> RequestRequirements {
        RequestRequirements {
            current_rules_ver: 3,
            min_reputation: 0.5,
            requires_tools: true,
            requires_json: true,
            min_ctx: 4096,
            input_tokens: 1000,
            output_tokens: 1000,
            max_price_mu: Some(100),
            now_millis: now,
            ..RequestRequirements::default()
        }
    }

    fn active_probation() -> ProviderProbation {
        ProviderProbation {
            active: true,
            since_seconds: 0,
            successful_sessions: 7,
            required_successful_sessions: 50,
            required_seconds: 7 * 24 * 60 * 60,
            caps: ProbationCaps {
                max_concurrent_sessions_per_user: 2,
                price_max_bps: 10_000,
                weight_bps: 5_000,
            },
        }
    }

    struct ScriptedRng {
        values: Vec<f64>,
        idx: usize,
    }

    impl ScriptedRng {
        fn new(values: impl IntoIterator<Item = f64>) -> Self {
            Self {
                values: values.into_iter().collect(),
                idx: 0,
            }
        }
    }

    impl BalancerRng for ScriptedRng {
        fn next_unit_f64(&mut self) -> f64 {
            let value = self.values[self.idx % self.values.len()];
            self.idx += 1;
            value
        }
    }

    #[test]
    fn provider_table_merges_contract_heartbeat_and_observed_ewma() {
        let now = 1_000_000;
        let mut table = ProviderTable::new();
        let record = contract_record();
        let key = ProviderKey::from_contract(&record);
        table.replace_contract_snapshot([record]);
        table.upsert_heartbeat(heartbeat(now, 7, "11"), now);

        table.record_observation(
            &key,
            ProviderObservationSample {
                ttft_ms: 200,
                tok_s: Some(40.0),
                error: false,
            },
        );
        table.record_observation(
            &key,
            ProviderObservationSample {
                ttft_ms: 100,
                tok_s: Some(80.0),
                error: true,
            },
        );

        let entries = table.entries(now + 500);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.contract.reputation, 0.72);
        assert_eq!(entry.heartbeat_age_millis, Some(500));
        assert_eq!(entry.heartbeat.as_ref().expect("live heartbeat").sat, 0.25);
        assert_eq!(entry.observed.samples, 2);
        assert_eq!(entry.observed.ewma_ttft_ms, Some(180.0));
        assert_eq!(entry.observed.ewma_tok_s, Some(48.0));
        assert!((entry.observed.ewma_error_rate - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn error_circuit_breaker_drops_provider_then_readmits_after_cooloff() {
        let now = 1_000_000;
        let mut table = ProviderTable::new();
        let record = contract_record_for(1);
        let key = ProviderKey::from_contract(&record);
        table.upsert_contract(record);
        table.upsert_heartbeat(heartbeat_for(1, now, 0.2, 100, 9, "44"), now);
        let request = eligible_request(now + 1);
        assert_eq!(
            eligible_candidates(
                &table.entries(now + 1),
                &request,
                &SelectionWeights::default()
            )
            .len(),
            1
        );

        for offset in 0..DEFAULT_ERROR_CIRCUIT_BREAKER_CONSECUTIVE_FAILURES {
            table.record_observation_at(
                &key,
                ProviderObservationSample {
                    ttft_ms: 5_000,
                    tok_s: None,
                    error: true,
                },
                now + u64::from(offset),
            );
        }

        let open_entries = table.entries(now + 3);
        let open_entry = open_entries.first().expect("provider entry");
        assert_eq!(
            evaluate_eligibility(open_entry, &eligible_request(now + 3)),
            Err(IneligibilityReason::CircuitOpen)
        );
        assert_eq!(
            eligible_candidates(
                &open_entries,
                &eligible_request(now + 3),
                &SelectionWeights::default()
            )
            .len(),
            0
        );

        let readmit_at = open_entry
            .observed
            .circuit_open_until_millis
            .expect("open circuit")
            + 1;
        table.upsert_heartbeat(heartbeat_for(1, readmit_at, 0.2, 100, 9, "44"), readmit_at);
        let readmitted_entries = table.entries(readmit_at + 1);
        let readmitted = readmitted_entries.first().expect("provider entry");
        assert_eq!(
            evaluate_eligibility(readmitted, &eligible_request(readmit_at + 1)),
            Ok(80)
        );

        table.record_observation_at(
            &key,
            ProviderObservationSample {
                ttft_ms: 100,
                tok_s: Some(50.0),
                error: false,
            },
            readmit_at + 1,
        );
        let recovered = table.entries(readmit_at + 2).pop().expect("provider entry");
        assert_eq!(recovered.observed.consecutive_failures, 0);
        assert_eq!(recovered.observed.circuit_open_until_millis, None);
    }

    #[test]
    fn provider_table_expires_heartbeats_after_ten_seconds() {
        let now = 1_000_000;
        let mut table = ProviderTable::new();
        table.upsert_contract(contract_record());
        table.upsert_heartbeat(heartbeat(now, 7, "11"), now);

        let live = table.entries(now + DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS);
        assert!(live[0].heartbeat.is_some());

        let expired = table.entries(now + DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS + 1);
        assert!(expired[0].heartbeat.is_none());
        assert_eq!(
            expired[0].heartbeat_age_millis,
            Some(DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS + 1)
        );
    }

    #[test]
    fn provider_table_caches_latest_attestation_head() {
        let now = 1_000_000;
        let mut table = ProviderTable::new();
        let key = key();

        table.upsert_heartbeat(heartbeat(now, 7, "11"), now);
        table.upsert_heartbeat(heartbeat(now + 1, 6, "22"), now + 1);
        assert_eq!(
            table
                .attestation_head(&key.provider, &key.enclave_id)
                .expect("cached head")
                .head,
            "11".repeat(32)
        );

        table.upsert_heartbeat(heartbeat(now + 2, 8, "33"), now + 2);
        let cached = table
            .attestation_head(&key.provider, &key.enclave_id)
            .expect("cached head");
        assert_eq!(cached.head, "33".repeat(32));
        assert_eq!(cached.epoch, 8);
        assert_eq!(cached.observed_at_millis, now + 2);
    }

    #[test]
    fn provider_table_does_not_route_noncanonical_heartbeats() {
        let mut table = ProviderTable::new();
        table.upsert_heartbeat(heartbeat(1_000_000, 7, "11"), 1_000_000);

        assert_eq!(table.heartbeat_len(), 1);
        assert!(table.entries(1_000_500).is_empty());
    }

    #[test]
    fn provider_table_ignores_noncanonical_contract_room_ids() {
        let mut table = ProviderTable::new();
        let mut record = contract_record();
        record.room_id = "provider-local-only".to_owned();

        table.upsert_contract(record);

        assert_eq!(table.contract_len(), 0);
        assert!(table.entries(1_000_500).is_empty());
    }

    #[test]
    fn provider_table_ignores_banned_contract_snapshots() {
        let mut table = ProviderTable::new();
        let mut record = contract_record();
        record.provider_status = Some("banned".to_owned());

        table.upsert_contract(record);

        assert_eq!(table.contract_len(), 0);
        assert!(table.entries(1_000_500).is_empty());
    }

    #[test]
    fn provider_table_ignores_banned_contract_snapshot_status_alias() {
        let mut record = serde_json::to_value(contract_record()).unwrap();
        record
            .as_object_mut()
            .expect("contract snapshot object")
            .remove("provider_status");
        record["status"] = serde_json::json!("banned");
        let record: ContractProviderSnapshot = serde_json::from_value(record).unwrap();

        let mut table = ProviderTable::new();
        table.upsert_contract(record);

        assert_eq!(table.contract_len(), 0);
        assert!(table.entries(1_000_500).is_empty());
    }

    #[test]
    fn provider_table_requires_active_contract_snapshot_status() {
        let mut table = ProviderTable::new();
        let mut record = contract_record();
        record.provider_status = None;

        table.upsert_contract(record);

        assert_eq!(table.contract_len(), 0);
        assert!(table.entries(1_000_500).is_empty());
    }

    #[test]
    fn eligibility_filter_applies_normative_predicates() {
        let now = 1_000_000;
        let request = eligible_request(now + 1);
        let good = entry_for(1, now, 0.2, 100);

        assert_eq!(evaluate_eligibility(&good, &request), Ok(80));

        let mut bad = good.clone();
        bad.contract.consent_ver = 2;
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::ConsentVersion)
        );

        let mut bad = good.clone();
        bad.contract.reputation = 0.4;
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::Reputation)
        );

        let mut bad = good.clone();
        bad.heartbeat = None;
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::HeartbeatMissing)
        );

        let mut bad = good.clone();
        bad.heartbeat_age_millis = Some(DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS);
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::HeartbeatStale)
        );

        let mut bad = good.clone();
        bad.heartbeat.as_mut().expect("heartbeat").sat = DEFAULT_SATURATION_CUTOFF;
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::Saturated)
        );

        let mut bad = good.clone();
        bad.heartbeat.as_mut().expect("heartbeat").caps.tools = false;
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::Capabilities)
        );

        let mut price_limited = request.clone();
        price_limited.max_price_mu = Some(79);
        assert_eq!(
            evaluate_eligibility(&good, &price_limited),
            Err(IneligibilityReason::Price)
        );

        let mut bad = good.clone();
        bad.attestation_head = None;
        assert_eq!(
            evaluate_eligibility(&bad, &request),
            Err(IneligibilityReason::AttestationMissing)
        );

        let mut bad = good;
        bad.attestation_head
            .as_mut()
            .expect("attestation head")
            .observed_at_millis = 0;
        let mut stale_attestation = request;
        stale_attestation.now_millis = DEFAULT_ATTESTATION_HEAD_MAX_AGE_MILLIS + 1;
        assert_eq!(
            evaluate_eligibility(&bad, &stale_attestation),
            Err(IneligibilityReason::AttestationStale)
        );
    }

    #[test]
    fn weighted_p2c_prefers_lower_saturation_then_ttft() {
        let now = 1_000_000;
        let request = eligible_request(now + 1);
        let weights = SelectionWeights::default();
        let entries = vec![entry_for(1, now, 0.7, 50), entry_for(2, now, 0.2, 500)];
        let mut rng = ScriptedRng::new([0.0, 0.0]);

        let selected =
            select_weighted_p2c(&entries, &request, &weights, &mut rng).expect("provider selected");
        assert_eq!(selected.sampled.len(), 2);
        assert_eq!(selected.selected.entry.key, entries[1].key);

        let entries = vec![entry_for(1, now, 0.2, 500), entry_for(2, now, 0.2, 100)];
        let mut rng = ScriptedRng::new([0.0, 0.0]);
        let selected =
            select_weighted_p2c(&entries, &request, &weights, &mut rng).expect("provider selected");
        assert_eq!(selected.selected.entry.key, entries[1].key);
    }

    #[test]
    fn weighted_p2c_prefers_material_anchored_reputation_gap() {
        let now = 1_000_000;
        let mut request = eligible_request(now + 1);
        request.min_reputation = 0.0;
        let weights = SelectionWeights::default();
        let mut low_reputation_fast = entry_for(1, now, 0.2, 50);
        low_reputation_fast.contract.reputation = 0.31;
        let mut healthy_slow = entry_for(2, now, 0.7, 500);
        healthy_slow.contract.reputation = 1.0;
        let entries = vec![low_reputation_fast, healthy_slow];
        let mut rng = ScriptedRng::new([0.0, 0.0]);

        let selected =
            select_weighted_p2c(&entries, &request, &weights, &mut rng).expect("provider selected");

        assert_eq!(selected.sampled.len(), 2);
        assert_eq!(selected.selected.entry.key, entries[1].key);
    }

    #[test]
    fn probation_caps_are_enforced_in_balancer() {
        let now = 1_000_000;
        let request = eligible_request(now + 1);
        let mut entry = entry_for(1, now, 0.2, 100);
        entry.contract.probation = Some(active_probation());

        assert_eq!(evaluate_eligibility(&entry, &request), Ok(80));

        let mut over_concurrent = request.clone();
        over_concurrent
            .provider_user_active_sessions
            .insert(entry.contract.provider.clone(), 2);
        assert_eq!(
            evaluate_eligibility(&entry, &over_concurrent),
            Err(IneligibilityReason::ProbationConcurrentLimit)
        );

        let mut overpriced = entry.clone();
        overpriced.contract.rate_map = text_generation_rate_map(21, 60);
        assert_eq!(
            evaluate_eligibility(&overpriced, &request),
            Err(IneligibilityReason::ProbationPriceCap)
        );

        let weights = SelectionWeights::default();
        let non_probation_weight = eligible_candidates(
            &[ProviderTableEntry {
                contract: ContractProviderSnapshot {
                    probation: None,
                    ..entry.contract.clone()
                },
                ..entry.clone()
            }],
            &request,
            &weights,
        )
        .pop()
        .expect("non-probation candidate")
        .weight;
        let probation_weight = eligible_candidates(&[entry], &request, &weights)
            .pop()
            .expect("probation candidate")
            .weight;
        assert!((probation_weight - non_probation_weight * 0.5).abs() < 1e-12);
    }

    #[test]
    fn weighted_p2c_synthetic_fleet_has_low_load_variance() {
        let now = 1_000_000;
        let request = eligible_request(now + 1);
        let weights = SelectionWeights::default();
        let entries = (0_u8..10)
            .map(|idx| entry_for(idx, now, 0.2, 120))
            .collect::<Vec<_>>();
        let mut rng = LcgBalancerRng::seeded(0xfeed_cafe);
        let mut counts = [0_u32; 10];

        for _ in 0..20_000 {
            let selected = select_weighted_p2c(&entries, &request, &weights, &mut rng)
                .expect("provider selected");
            let idx = entries
                .iter()
                .position(|entry| entry.key == selected.selected.entry.key)
                .expect("selected provider in fleet");
            counts[idx] += 1;
        }

        let mean = counts.iter().map(|count| f64::from(*count)).sum::<f64>() / counts.len() as f64;
        let variance = counts
            .iter()
            .map(|count| {
                let delta = f64::from(*count) - mean;
                delta * delta
            })
            .sum::<f64>()
            / counts.len() as f64;
        let coefficient_of_variation = variance.sqrt() / mean;
        let max_share = f64::from(*counts.iter().max().expect("max count")) / 20_000.0;

        assert!(
            coefficient_of_variation < 0.06,
            "counts={counts:?}, cv={coefficient_of_variation}"
        );
        assert!(max_share < 0.115, "counts={counts:?}");
    }
}
