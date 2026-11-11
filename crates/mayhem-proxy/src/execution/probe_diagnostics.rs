//! Optional failure-only metadata: at most 64 records per controller process.
//! No request/response content, vendor messages, URLs or credentials.
use super::*;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    OnceLock,
};

fn take(counter: &AtomicU32) -> bool {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < 64).then(|| n + 1)
        })
        .is_ok()
}

fn record(probe: &capacity::probes::Probe, error: &Error, ended: bool) -> serde_json::Value {
    let (kind, upstream) = match error {
        Error::Upstream(f) | Error::Decoder(worker::Error::Upstream(f)) => ("upstream", Some(f)),
        Error::Decoder(worker::Error::ProcessingTimeout) => ("decoder_timeout", None),
        Error::Decoder(worker::Error::Identity) => ("decoder_identity", None),
        Error::Decoder(_) => ("decoder", None),
        Error::Endpoint(_) => ("endpoint_validation", None),
        Error::Cancelled => ("cancelled", None),
        Error::TransportWorker => ("transport_worker", None),
        Error::StorageWorker | Error::StorageCapacity | Error::Journal(_) => ("storage", None),
        Error::Financial(_) => ("financial", None),
        Error::Capacity(_) => ("capacity", None),
        _ => ("execution", None),
    };
    serde_json::json!({
        "event":"proxy_probe_failure", "probe":probe.id, "route":probe.specification.route,
        "kind":kind, "upstream":upstream, "execution_ended":ended,
    })
}

pub(super) fn report(probe: &capacity::probes::Probe, error: &Error, ended: bool) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static COUNT: AtomicU32 = AtomicU32::new(0);
    if *ENABLED.get_or_init(|| std::env::var("MAYHEM_PROXY_DIAGNOSTICS").as_deref() == Ok("1"))
        && take(&COUNT)
    {
        eprintln!("{}", record(probe, error, ended));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_records_have_a_fixed_process_budget() {
        let counter = AtomicU32::new(0);
        for _ in 0..64 {
            assert!(take(&counter));
        }
        for _ in 0..1_000 {
            assert!(!take(&counter));
        }
        assert_eq!(counter.load(Ordering::Relaxed), 64);
    }
    #[test]
    fn failure_record_does_not_include_arbitrary_error_messages() {
        let digest = attempts::Digest::new("00".repeat(32)).unwrap();
        let probe = capacity::probes::Probe {
            id: digest.clone(),
            controller_fence: 1,
            phase: capacity::probes::ProbePhase::Dispatched,
            allocated_cost_microusd: 0,
            specification: capacity::probes::Specification {
                route: digest.clone(),
                budget_group: digest.clone(),
                request_hash: digest.clone(),
                connection_digest: digest.clone(),
                connection_revision: 1,
                recipe_digest: digest,
            },
        };
        let value = record(
            &probe,
            &Error::Financial(crate::invalid("secret prompt https://private/key")),
            false,
        );
        let encoded = value.to_string();
        assert_eq!(value["kind"], "financial");
        assert!(!encoded.contains("secret") && !encoded.contains("private"));
        assert!(encoded.len() < 512);
    }
}
