//! Opt-in bounded verification metadata. Never retains request or response data.
use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LIMIT: u8 = 64;
const WINDOW: Duration = Duration::from_secs(600);
static BUDGET: Mutex<Budget> = Mutex::new(Budget {
    first: None,
    emitted: 0,
});
struct Budget {
    first: Option<Instant>,
    emitted: u8,
}
impl Budget {
    fn take(&mut self, now: Instant) -> bool {
        let first = *self.first.get_or_insert(now);
        if self.emitted >= LIMIT || now.saturating_duration_since(first) >= WINDOW {
            return false;
        }
        self.emitted += 1;
        true
    }
}

fn reason(message: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        "provider partial receipt prompt usage mismatch",
        "provider receipt amount mismatch",
        "provider receipt billing epoch mismatch",
        "provider receipt billing prior amount mismatch",
        "provider receipt billing prior usage mismatch",
        "provider receipt billing_attempt mismatch",
        "provider receipt billing_id mismatch",
        "provider receipt cached output usage mismatch",
        "provider receipt cached prompt usage mismatch",
        "provider receipt ctx_bracket mismatch",
        "provider receipt ctx_bracket_table_ver mismatch",
        "provider receipt enclave mismatch",
        "provider receipt finality mismatch",
        "provider receipt locked_min_session_au mismatch",
        "provider receipt locked_per_req_au mismatch",
        "provider receipt locked_rate_map mismatch",
        "provider receipt model mismatch",
        "provider receipt payout revision mismatch",
        "provider receipt price_ver mismatch",
        "provider receipt prompt_hash mismatch",
        "provider receipt provider mismatch",
        "provider receipt rail mismatch",
        "provider receipt reservation id mismatch",
        "provider receipt rules_ver mismatch",
        "provider receipt seq mismatch",
        "provider receipt served_ctx mismatch",
        "provider receipt session_id mismatch",
        "provider receipt usage mismatch",
        "provider receipt user mismatch",
        "provider receipt workflow binding mismatch",
        "provider receipt workflow output binding mismatch",
        "receipt settlement handoff session or sequence mismatch",
        "tools-only provider receipt output usage mismatch",
        "provider receipt enclave signature failed",
        "provider accept signature failed",
        "provider accept attestation verification failed",
        "receipt settlement provider_sig verification failed",
        "receipt acknowledgement signature verification failed",
        "receipt exceeds signed spend voucher",
        "usage exceeded signed spend voucher",
    ];
    KNOWN
        .iter()
        .copied()
        .find(|known| message.contains(known))
        .unwrap_or("unclassified_verification_failure")
}

pub(super) fn record(message: &str) {
    let Some(path) = std::env::var_os("MAYHEM_GATEWAY_VERIFICATION_DIAGNOSTICS_FILE") else {
        return;
    };
    if !Path::new(&path).is_file() {
        return;
    }
    let Ok(mut budget) = BUDGET.lock() else {
        return;
    };
    if !budget.take(Instant::now()) {
        return;
    }
    drop(budget);
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    eprintln!(
        "[gateway-verification] at_ms={at} reason={}",
        reason(message)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_is_allowlisted_and_cannot_echo_payloads() {
        let private = "SECRET_PROMPT_AND_CREDENTIAL";
        assert_eq!(reason(private), "unclassified_verification_failure");
        assert_eq!(
            reason(&format!(
                "provider partial receipt prompt usage mismatch: {private}"
            )),
            "provider partial receipt prompt usage mismatch"
        );
        assert_eq!(
            reason(&format!(
                "receipt settlement provider_sig verification failed: {private}"
            )),
            "receipt settlement provider_sig verification failed"
        );
        assert!(!reason(private).contains(private));
    }
    #[test]
    fn budget_stops_after_64_events_or_ten_minutes() {
        let now = Instant::now();
        let mut b = Budget {
            first: None,
            emitted: 0,
        };
        for _ in 0..LIMIT {
            assert!(b.take(now));
        }
        assert!(!b.take(now));
        let mut b = Budget {
            first: None,
            emitted: 0,
        };
        assert!(b.take(now));
        assert!(!b.take(now + WINDOW));
        assert!(!b.take(now + WINDOW + Duration::from_secs(1)));
    }
}
