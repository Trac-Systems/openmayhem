use super::*;
use ed25519_dalek::{Signer, SigningKey};
use mayhem_proto::proxy::finance::{
    ProxyExpiryBody, ProxyHoldExpiry, ProxyReservationExpiry, ProxyUsageReceipt,
};
use mayhem_proxy::attempts::{ResultCommitment, TerminalDraft};
use serde_json::json;

fn digest(n: u8) -> Digest {
    Digest::new(format!("{n:02x}").repeat(32)).unwrap()
}
fn sign(seed: u8, bytes: &[u8]) -> String {
    hex::encode(SigningKey::from_bytes(&[seed; 32]).sign(bytes).to_bytes())
}
fn fixture() -> (ProxySpendTerms, ProxySettlementPolicy) {
    let data: Value = serde_json::from_str(include_str!(
        "../../../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
    ))
    .unwrap();
    let mut terms: ProxySpendTerms =
        serde_json::from_value(data["cases"][0]["terms"].clone()).unwrap();
    let mut policy: ProxySettlementPolicy =
        serde_json::from_value(data["cases"][0]["policy"].clone()).unwrap();
    terms.buyer_pubkey = hex::encode(SigningKey::from_bytes(&[41; 32]).verifying_key().to_bytes());
    terms.offer.provider_pubkey =
        hex::encode(SigningKey::from_bytes(&[42; 32]).verifying_key().to_bytes());
    policy.hold_expiry = Some(ProxyHoldExpiry::ReleaseUnfinalizedAndBlockRetry);
    terms.settlement_policy_hash = policy.digest().unwrap();
    terms.validate().unwrap();
    (terms, policy)
}
fn identity(terms: &ProxySpendTerms) -> RequestIdentity {
    RequestIdentity {
        billing_id: Digest::new(&terms.billing_id).unwrap(),
        billing_attempt: terms.billing_attempt,
        session_id: Digest::new(&terms.session_id).unwrap(),
        request_hash: Digest::new(&terms.request_hash).unwrap(),
    }
}
fn model(terms: &ProxySpendTerms) -> String {
    format!(
        "proxy/offer/{}/{}/{}",
        terms.offer.market_id,
        terms.offer.provider_pubkey,
        terms.offer.slot_id().unwrap()
    )
}
fn open(root: &Path, jobs: usize, bytes: usize, now: u64) -> GatewayJobStore {
    GatewayJobStore::durable([7; 32], root.to_owned(), jobs, bytes, 2, now).unwrap()
}
fn begin(
    store: &mut GatewayJobStore,
    id: &str,
    terms: &ProxySpendTerms,
) -> Result<BeginGatewayJob, String> {
    store.begin_proxy(
        id.to_owned(),
        "openai_chat_completions".into(),
        model(terms),
        Some("owner".into()),
        digest(9).as_str().into(),
        identity(terms),
        1,
    )
}
fn authorized(store: &mut GatewayJobStore, id: &str) -> (ProxySpendTerms, ProxySettlementPolicy) {
    let (terms, policy) = fixture();
    begin(store, id, &terms).unwrap();
    store
        .authorize_proxy_terms(id, terms.clone(), policy.clone(), 2)
        .unwrap();
    (terms, policy)
}
fn output(terms: &ProxySpendTerms) -> ProxyVerifiedOutput {
    let authorization = ProxySpendAuthorization {
        terms: terms.clone(),
        buyer_sig: sign(41, &terms.buyer_signing_bytes().unwrap()),
        provider_sig: sign(42, &terms.provider_signing_bytes().unwrap()),
    };
    let invocation = mayhem_proxy::exchange::invocation(&authorization).unwrap();
    let data: Value = serde_json::from_str(include_str!(
        "../../../../mayhem-proto/tests/fixtures/proxy-finance-v1.json"
    ))
    .unwrap();
    let mut body: mayhem_proto::proxy::finance::ProxyReceiptBody =
        serde_json::from_value(data["cases"][0]["receipt"].clone()).unwrap();
    body.accepted_terms = terms.digest().unwrap();
    body.seq = 1;
    let receipt = ProviderReceipt {
        provider_sig: sign(42, &body.provider_signing_bytes().unwrap()),
        draft: TerminalDraft {
            invocation: invocation.clone(),
            attempt: 1,
            body,
            previous: None,
            result_commitment: ResultCommitment::PublicV1,
        },
    };
    ProxyVerifiedOutput {
        identity: identity(terms),
        authorization,
        response: json!({"id":format!("proxy_{}", invocation.as_str()), "private_output":"fixture-only response"}),
        receipt,
    }
}
fn proof() -> Proof {
    Proof {
        view_key: digest(1).as_str().into(),
        fork: 0,
        signed_length: 4,
        tree_hash: digest(2).as_str().into(),
    }
}
// Exercise the private disk state independently of RPC. Production close_proxy
// accepts only the opaque authenticated Observation and extracts its verified outcome.
fn stage_paid_closure(store: &mut GatewayJobStore, id: &str) -> Digest {
    let mut job = store.proxy_job(id).unwrap();
    let state = job.proxy.as_mut().unwrap();
    let provider = state.output_receipt.as_ref().unwrap();
    let receipt = ProxyUsageReceipt {
        body: provider.draft.body.clone(),
        provider_sig: provider.provider_sig.clone(),
        buyer_sig: sign(41, &provider.draft.body.buyer_signing_bytes().unwrap()),
    };
    let closure = ProxyClosure {
        outcome: FinancialOutcome::Paid { receipt },
        proof: proof(),
    };
    let marker = closure.commitment().unwrap();
    state.closure = Some(closure);
    store.persist_proxy(job).unwrap();
    marker
}

#[test]
fn proxy_non_admission_retains_pending_fence_until_matching_budget_commit_and_blocks_signing() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, _) = authorized(&mut store, "job_fenced");
    let fence = ProxyNonAdmission {
        identity: identity(&terms),
        terms: Digest::new(terms.digest().unwrap()).unwrap(),
        maximum: terms.max_spend_au,
        marker: digest(33),
    };
    let mut wrong = fence.clone();
    wrong.maximum += 1;
    assert!(store
        .retain_proxy_non_admission("job_fenced", wrong, 3)
        .is_err());
    store
        .retain_proxy_non_admission("job_fenced", fence.clone(), 3)
        .unwrap();
    assert_eq!(store.pending_proxy(None, 64).unwrap(), ["job_fenced"]);
    assert!(store
        .finish_proxy_non_admission("job_fenced", &digest(34), 4)
        .is_err());
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 1000);
    assert_eq!(
        store
            .get("job_fenced", 1000)
            .unwrap()
            .unwrap()
            .proxy
            .unwrap()
            .phase(),
        ProxyPhase::NonAdmissionPendingBudget
    );
    let (_, policy) = fixture();
    assert!(store
        .authorize_proxy_terms("job_fenced", terms.clone(), policy, 1000)
        .is_err());
    assert!(store
        .retain_proxy_output("job_fenced", output(&terms), 1000)
        .is_err());
    store
        .finish_proxy_non_admission("job_fenced", &fence.marker, 1000)
        .unwrap();
    store
        .finish_proxy_non_admission("job_fenced", &fence.marker, 1000)
        .unwrap();
    assert!(store.pending_proxy(None, 64).unwrap().is_empty());
    assert_eq!(
        store
            .get("job_fenced", 1000)
            .unwrap()
            .unwrap()
            .proxy
            .unwrap()
            .phase(),
        ProxyPhase::NotAdmitted
    );
}

#[test]
fn proxy_failure_before_authorization_reopens_terminal_but_cannot_retire_held_intent() {
    let root = tempfile::tempdir().unwrap();
    let (terms, policy) = fixture();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    begin(&mut store, "job_unsigned", &terms).unwrap();
    let job = store
        .fail_proxy_before_authorization("job_unsigned", 2)
        .unwrap();
    assert_eq!(job.status, GatewayJobStatus::Failed);
    assert_eq!(job.proxy.unwrap().phase(), ProxyPhase::NotAuthorized);
    assert!(store.pending_proxy(None, 4).unwrap().is_empty());
    assert!(store
        .authorize_proxy_terms("job_unsigned", terms.clone(), policy.clone(), 2)
        .is_err());
    begin(&mut store, "job_held", &terms).unwrap();
    store
        .authorize_proxy_terms("job_held", terms, policy, 2)
        .unwrap();
    assert!(store
        .fail_proxy_before_authorization("job_held", 2)
        .is_err());
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 2);
    assert_eq!(
        store.get("job_unsigned", 2).unwrap().unwrap().status,
        GatewayJobStatus::Failed
    );
    assert_eq!(store.pending_proxy(None, 4).unwrap(), ["job_held"]);
}

#[test]
fn proxy_authorization_reopens_pinned_with_original_identity_and_no_journal_claim() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, policy) = authorized(&mut store, "job_proxy");
    assert_eq!(
        store
            .authorize_proxy_terms("job_proxy", terms.clone(), policy, 10)
            .unwrap()
            .proxy
            .unwrap()
            .phase(),
        ProxyPhase::AwaitingBuyerJournal
    );
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 1_000_000);
    let job = store.get("job_proxy", 1_000_000).unwrap().unwrap();
    let state = job.proxy.unwrap();
    assert_eq!(state.identity(), &identity(&terms));
    assert_eq!(state.terms().unwrap().max_spend_au, terms.max_spend_au);
    assert_eq!(state.phase(), ProxyPhase::AwaitingBuyerJournal);
    assert_eq!(store.pending_proxy(None, 1).unwrap(), ["job_proxy"]);
    assert!(store.remove("job_proxy", Some("owner"), 1_000_000).is_err());
    assert!(store
        .finish_proxy("job_proxy", digest(1), 1_000_000)
        .is_err());
    assert!(matches!(
        begin(&mut store, "job_proxy", &terms).unwrap(),
        BeginGatewayJob::Existing(_)
    ));
    let mut changed = identity(&terms);
    changed.session_id = digest(3);
    assert!(store
        .begin_proxy(
            "job_proxy".into(),
            "openai_chat_completions".into(),
            model(&terms),
            Some("owner".into()),
            digest(9).as_str().into(),
            changed,
            4
        )
        .is_err());
    assert!(store
        .begin_proxy(
            "job_proxy".into(),
            "openai_chat_completions".into(),
            model(&terms),
            Some("other-owner".into()),
            digest(9).as_str().into(),
            identity(&terms),
            4
        )
        .is_err());
    assert!(store
        .begin_proxy(
            "job_proxy".into(),
            "openai_chat_completions".into(),
            model(&terms),
            Some("owner".into()),
            digest(8).as_str().into(),
            identity(&terms),
            4
        )
        .is_err());
}

#[test]
fn proxy_output_is_encrypted_immutable_and_retained_until_exact_budget_finish() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, _) = authorized(&mut store, "job_output");
    let first = store
        .retain_proxy_output("job_output", output(&terms), 3)
        .unwrap();
    assert_eq!(
        first.proxy.as_ref().unwrap().phase(),
        ProxyPhase::OutputRetained
    );
    assert_eq!(
        first,
        store
            .retain_proxy_output("job_output", output(&terms), 100)
            .unwrap()
    );
    let mut changed = output(&terms);
    changed.response["private_output"] = json!("different");
    assert!(store.retain_proxy_output("job_output", changed, 4).is_err());
    let bytes = fs::read(job_path(root.path(), "job_output")).unwrap();
    assert!(!bytes
        .windows(b"fixture-only response".len())
        .any(|v| v == b"fixture-only response"));
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 1000);
    assert_eq!(
        store.get("job_output", 1000).unwrap().unwrap().result,
        first.result
    );
    let marker = stage_paid_closure(&mut store, "job_output");
    assert_eq!(
        store
            .get("job_output", 1000)
            .unwrap()
            .unwrap()
            .proxy
            .unwrap()
            .phase(),
        ProxyPhase::ClosurePendingBudget
    );
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 1001);
    assert_eq!(store.pending_proxy(None, 64).unwrap(), ["job_output"]);
    assert!(store.finish_proxy("job_output", digest(8), 1002).is_err());
    let closed = store
        .finish_proxy("job_output", marker.clone(), 1002)
        .unwrap();
    assert_eq!(closed.status, GatewayJobStatus::Completed);
    assert!(store.pending_proxy(None, 64).unwrap().is_empty());
    assert_eq!(
        closed,
        store.finish_proxy("job_output", marker, 1003).unwrap()
    );
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 1003);
    assert_eq!(store.get("job_output", 1003).unwrap().unwrap(), closed);
}

#[test]
fn proxy_pending_count_byte_bounds_and_cursor_never_evict_obligations() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 3, 1024 * 1024, 1);
    let (terms, _) = fixture();
    for id in ["job_c", "job_a", "job_b"] {
        begin(&mut store, id, &terms).unwrap();
    }
    assert_eq!(store.pending_proxy(None, 2).unwrap(), ["job_a", "job_b"]);
    assert_eq!(store.pending_proxy(Some("job_b"), 2).unwrap(), ["job_c"]);
    assert!(store.pending_proxy(None, 0).is_err());
    assert!(store.pending_proxy(None, 65).is_err());
    assert!(begin(&mut store, "job_d", &terms).is_err());
    assert!(store
        .begin(
            "job_native".into(),
            "image".into(),
            "native".into(),
            None,
            "native".into(),
            1000
        )
        .is_err());
    let before = store.pending_proxy(None, 64).unwrap();
    let retained = store.total_bytes;
    store.max_bytes = retained;
    let (_, policy) = fixture();
    assert!(store
        .authorize_proxy_terms("job_a", terms, policy, 1000)
        .is_err());
    assert_eq!(store.pending_proxy(None, 64).unwrap(), before);
    assert_eq!(store.total_bytes, retained);
    drop(store);
    assert!(
        GatewayJobStore::durable([7; 32], root.path().to_owned(), 2, 1024 * 1024, 2, 2000).is_err()
    );
    let store = open(root.path(), 3, 1024 * 1024, 2000);
    assert_eq!(store.pending_proxy(None, 64).unwrap(), before);
}

#[test]
fn proxy_failed_write_ack_requires_reopen_and_retains_committed_authorization() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, policy) = fixture();
    begin(&mut store, "job_crash", &terms).unwrap();
    store.proxy_lose_write_ack = true;
    assert!(store
        .authorize_proxy_terms("job_crash", terms.clone(), policy.clone(), 2)
        .is_err());
    assert!(store.proxy_failed);
    assert!(store
        .authorize_proxy_terms("job_crash", terms.clone(), policy.clone(), 2)
        .is_err());
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 1000);
    let recovered = store
        .authorize_proxy_terms("job_crash", terms, policy, 1000)
        .unwrap();
    assert_eq!(
        recovered.proxy.unwrap().phase(),
        ProxyPhase::AwaitingBuyerJournal
    );
}

#[test]
fn proxy_and_native_reconciliation_cannot_reinterpret_each_others_records() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, _) = authorized(&mut store, "job_proxy");
    assert!(store.pending_reconciliations(1000).unwrap().is_empty());
    assert!(store
        .begin(
            "job_proxy".into(),
            "openai_chat_completions".into(),
            model(&terms),
            Some("owner".into()),
            digest(9).as_str().into(),
            1000
        )
        .is_err());
    assert!(store
        .complete(
            "job_proxy",
            GatewayJobStatus::Completed,
            None,
            vec![],
            None,
            None,
            1000
        )
        .is_err());
    assert!(store
        .finish_reconciliation("job_proxy", GatewayJobStatus::Failed, None, 1000)
        .is_err());
    assert!(store
        .update_reconciliation_receipt("job_proxy", json!({"body": {}}), 1000)
        .is_err());
    store
        .begin(
            "job_native".into(),
            "image".into(),
            "native".into(),
            None,
            "native".into(),
            1,
        )
        .unwrap();
    store.protect_active("job_native", 1).unwrap();
    store
        .update_active_receipt("job_native", json!({"native": true}), 2)
        .unwrap();
    store
        .complete(
            "job_native",
            GatewayJobStatus::ReconciliationPending,
            None,
            vec![],
            Some(json!({"native": true})),
            None,
            2,
        )
        .unwrap();
    assert!(begin(&mut store, "job_native", &terms).is_err());
    assert_eq!(store.pending_reconciliations(1000).unwrap().len(), 1);
    assert_eq!(store.pending_proxy(None, 64).unwrap(), ["job_proxy"]);
    let native = store.get("job_native", 1000).unwrap().unwrap();
    let mut legacy = serde_json::to_value(native).unwrap();
    legacy.as_object_mut().unwrap().remove("proxy");
    assert!(serde_json::from_value::<StoredGatewayJob>(legacy)
        .unwrap()
        .proxy
        .is_none());
}

#[test]
fn proxy_expiry_finishes_unknown_without_authorizing_retry_or_claiming_output() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, _) = authorized(&mut store, "job_expiry");
    let auth = output(&terms).authorization;
    let body = ProxyExpiryBody {
        schema_version: 1,
        lane: mayhem_proto::proxy::ProxyLane::Proxy,
        accepted_terms: terms.digest().unwrap(),
        observed_epoch: 100,
        at_ms: 100,
    };
    let expiry = ProxyReservationExpiry {
        buyer_sig: sign(41, &body.buyer_signing_bytes().unwrap()),
        body,
    };
    let closure = ProxyClosure {
        outcome: FinancialOutcome::ExpiredUnknown { expiry },
        proof: proof(),
    };
    let marker = closure.commitment().unwrap();
    let mut job = store.proxy_job("job_expiry").unwrap();
    let state = job.proxy.as_mut().unwrap();
    state.authorization = Some(auth);
    state.buyer_journal_seen = true;
    state.closure = Some(closure);
    store.persist_proxy(job).unwrap();
    let job = store.finish_proxy("job_expiry", marker, 10).unwrap();
    assert_eq!(job.status, GatewayJobStatus::Failed);
    assert!(job.result.is_none());
    assert!(!job.error_info.as_ref().unwrap().retryable);
    assert_eq!(
        job.error_info.as_ref().unwrap().category,
        "execution_unknown"
    );
}

#[test]
fn proxy_rejects_foreign_output_altered_terms_and_unprotected_storage() {
    let (terms, policy) = fixture();
    let mut memory = GatewayJobStore::in_memory([7; 32], 4, 1024 * 1024, 2);
    assert!(begin(&mut memory, "job_memory", &terms).is_err());
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    authorized(&mut store, "job_bound");
    let mut changed = terms.clone();
    changed.max_total_spend_au += 1;
    assert!(store
        .authorize_proxy_terms("job_bound", changed, policy, 2)
        .is_err());
    let mut foreign = output(&terms);
    foreign.identity.billing_id = digest(55);
    assert!(store.retain_proxy_output("job_bound", foreign, 2).is_err());
    let mut forged = output(&terms);
    forged.receipt.provider_sig = "0".repeat(128);
    assert!(store.retain_proxy_output("job_bound", forged, 2).is_err());
    assert!(store.get("job_bound", 3).unwrap().unwrap().result.is_none());
}
