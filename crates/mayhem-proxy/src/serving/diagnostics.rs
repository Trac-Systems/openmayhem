//! Optional, bounded session diagnostics. No request content, URLs, credentials
//! or arbitrary error messages are logged. Disabled unless explicitly enabled.
use std::{
    error::Error,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
};

fn details(error: &(dyn Error + 'static)) -> (&'static str, Option<String>) {
    if let Some(error) = error.downcast_ref::<mayhem_bridge::BridgeError>() {
        use mayhem_bridge::BridgeError as B;
        let code = match error {
            B::Timeout => "timeout",
            B::Closed => "closed",
            B::Protocol(_) => "protocol",
            B::ResourceLimit(_) => "resource_limit",
            B::WebSocket(_) => "websocket",
            B::Http(_) => "http",
            B::Json(_) => "json",
            B::InvalidUrl(_) => "url",
            B::RpcStatus { .. } => "rpc_status",
        };
        return ("bridge", Some(code.into()));
    }
    if let Some(error) = error.downcast_ref::<crate::Error>() {
        return match error {
            crate::Error::Invalid(message) => (
                "validation",
                Some(blake3::hash(message.as_bytes()).to_hex().to_string()),
            ),
            crate::Error::Http { status, .. } => ("financial_http", Some(status.to_string())),
            crate::Error::Transport(_) => ("financial_transport", None),
            crate::Error::FinancialAdmissionUnavailable => ("financial_unavailable", None),
            _ => ("control", None),
        };
    }
    if let Some(error) = error.downcast_ref::<crate::attempts::Error>() {
        // This enum contains no caller-controlled data.
        return ("journal", Some(format!("{error:?}")));
    }
    if let Some(error) = error.downcast_ref::<crate::capacity::Error>() {
        return ("capacity", Some(format!("{error:?}")));
    }
    if let Some(source) = error.source() {
        return details(source);
    }
    if error.is::<crate::execution::Error>() {
        ("execution", None)
    } else if error.is::<crate::exchange::Error>() {
        ("exchange", None)
    } else if error.is::<mayhem_bridge::BridgeError>() {
        ("bridge", None)
    } else {
        ("session", None)
    }
}

pub(crate) fn report(stage: &'static str, error: &(dyn Error + 'static)) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);
    static COUNT: AtomicU64 = AtomicU64::new(0);
    if !*ENABLED.get_or_init(|| std::env::var("MAYHEM_PROXY_DIAGNOSTICS").as_deref() == Ok("1")) {
        return;
    }
    COUNT.fetch_add(1, Ordering::Relaxed);
    let now = crate::supervisor::unix_ms();
    let old = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(old) < 30_000
        || LAST
            .compare_exchange(old, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let (kind, detail) = details(error);
    eprintln!(
        "{}",
        serde_json::json!({"event":"proxy_session_failure","stage":stage,"kind":kind,"detail":detail,"count":COUNT.swap(0, Ordering::Relaxed)})
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_errors_preserve_identity_without_disclosing_raw_data() {
        let private = "private prompt https://example.invalid/secret?token=private";
        let error = crate::serving::Error::Transport(crate::exchange::Error::Execution(
            crate::execution::Error::Financial(crate::invalid(private)),
        ));
        let (kind, detail) = details(&error);
        assert_eq!(kind, "validation");
        assert_eq!(
            detail.unwrap(),
            blake3::hash(private.as_bytes()).to_hex().to_string()
        );
        let http = crate::Error::Http {
            status: 503,
            code: private.into(),
        };
        assert_eq!(details(&http), ("financial_http", Some("503".into())));
        assert_eq!(
            details(&crate::Error::FinancialAdmissionUnavailable),
            ("financial_unavailable", None)
        );
        let admission =
            crate::execution::Error::Financial(crate::Error::FinancialAdmissionUnavailable);
        assert_eq!(details(&admission), ("financial_unavailable", None));
        let bridge =
            crate::exchange::Error::Transport(mayhem_bridge::BridgeError::Protocol(private.into()));
        assert_eq!(details(&bridge), ("bridge", Some("protocol".into())));
        assert_eq!(
            details(&mayhem_bridge::BridgeError::Timeout),
            ("bridge", Some("timeout".into()))
        );
    }
}
