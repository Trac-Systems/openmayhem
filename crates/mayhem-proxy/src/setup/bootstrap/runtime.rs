//! Reviewed resource preset for one connection. No price, charging outcome,
//! allowance, context, concurrency or credential is supplied by this preset.
use super::*;
use crate::{
    attempts, health, negotiation, serving, supervisor::RefreshPolicy, worker::host::PoolLimits,
};
use std::time::Duration;

pub(super) fn endpoint_limits() -> crate::endpoint::Limits {
    crate::endpoint::Limits {
        request_bytes: 1024 * 1024,
        response_bytes: 4 * 1024 * 1024,
        choices: 16,
        tools: 64,
        questions: 64,
        decision_options: 64,
    }
}
pub(super) fn template(
    host: &Host,
    tokenizer: Option<managed::Tokenizer>,
    concurrency: u32,
    closed_retention_ms: u64,
    allow_recovery_probes: bool,
) -> RunTemplate {
    let sessions = concurrency as usize;
    let pool = PoolLimits {
        max_children: sessions.min(32),
        max_buffer_bytes: 256 * 1024 * 1024,
        startup_timeout: Duration::from_secs(10),
        processing_timeout: Duration::from_secs(30),
    };
    RunTemplate {
        schema_version: 1,
        bridge: managed::Bridge {
            url: host.bridge_url.clone(),
            token_file: host.bridge_token_file.clone(),
            operation_timeout_ms: 15000,
            frame_bytes: 1024 * 1024,
            queue_events: 64,
            queue_bytes: 16 * 1024 * 1024,
            logical_message_bytes: 8 * 1024 * 1024,
        },
        health: health::Policy {
            max_routes: 1,
            max_classes_per_route: 32,
            evidence_ttl_ms: 60_000,
            successes_to_increase: 2,
            bad_samples_to_reduce: 2,
            latency_baseline_samples: 3,
            latency_multiplier: 4,
            latency_increase_ms: 1000,
            min_native_tok_s: 5,
            recovery: RefreshPolicy::default(),
        },
        limits: managed::Limits {
            max_groups: 8,
            max_routes: 1,
            max_leases: 65536,
            tokenizer_bytes: 64 * 1024 * 1024,
            tokenizer_workers: 16,
            financial_reads: 8,
            sessions,
            registrations: 16,
            observation_ms: 1000,
            recovery_interval_ms: 30_000,
            serving: serving::Limits {
                sessions,
                per_buyer: sessions,
                outbound_messages: 64,
                outbound_bytes: 16 * 1024 * 1024,
                control_wait: Duration::from_secs(30),
                proposals: negotiation::provider::Limits {
                    pending: sessions,
                    per_buyer: sessions,
                    request_bytes: 1024 * 1024,
                    total_request_bytes: 16 * 1024 * 1024,
                    storage_operations: 8,
                    unsigned_lifetime: Duration::from_secs(30),
                },
            },
            worker: pool,
            recovery_worker: PoolLimits {
                max_children: 1,
                ..pool
            },
            settlement_worker: PoolLimits {
                max_children: 2,
                ..pool
            },
            journal: attempts::Limits {
                max_records: 100_000,
                max_unfinished: 65536,
                closed_retention_ms,
                max_payload_bytes: 1024 * 1024 * 1024,
            },
            maintenance: serving::maintenance::Policy {
                page_size: 32,
                schedule: RefreshPolicy::default(),
            },
        },
        tokenizer,
        allow_recovery_probes,
    }
}
