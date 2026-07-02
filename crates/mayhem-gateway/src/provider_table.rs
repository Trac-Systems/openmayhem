use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{HeartbeatCaps, ProviderHeartbeat};

pub const DEFAULT_PROVIDER_HEARTBEAT_TTL_MILLIS: u64 = 10_000;
pub const DEFAULT_OBSERVATION_EWMA_ALPHA: f64 = 0.2;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ProviderKey {
    pub provider: String,
    pub enclave_id: String,
    pub room_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContractProviderSnapshot {
    pub provider: String,
    pub enclave_id: String,
    pub model_id: String,
    pub room_id: String,
    pub consent_ver: u64,
    pub reputation: f64,
    pub price_ver: u64,
    pub in_per_1k_mu: u64,
    pub out_per_1k_mu: u64,
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

fn update_ewma(current: Option<f64>, sample: f64, alpha: f64) -> f64 {
    match current {
        Some(current) => alpha * sample + (1.0 - alpha) * current,
        None => sample,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeartbeatAttestation, HeartbeatPerf, HeartbeatQueue, HeartbeatSlots};

    fn key() -> ProviderKey {
        ProviderKey::new("aa".repeat(32), "bb".repeat(32), "cc".repeat(16))
    }

    fn caps() -> HeartbeatCaps {
        HeartbeatCaps {
            tools: true,
            json: true,
            ctx: 8192,
            vision: false,
        }
    }

    fn contract_record() -> ContractProviderSnapshot {
        let key = key();
        ContractProviderSnapshot {
            provider: key.provider,
            enclave_id: key.enclave_id,
            model_id: "model/test@4bit".to_owned(),
            room_id: key.room_id,
            consent_ver: 3,
            reputation: 0.72,
            price_ver: 5,
            in_per_1k_mu: 20,
            out_per_1k_mu: 60,
            caps: caps(),
            attestation_head: None,
        }
    }

    fn heartbeat(ts: u64, epoch: u64, head: &str) -> ProviderHeartbeat {
        let key = key();
        ProviderHeartbeat {
            t: "hb".to_owned(),
            v: crate::HEARTBEAT_SCHEMA_VERSION,
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
}
