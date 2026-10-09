use super::*;

fn public(job: &StoredGatewayJob) -> Value {
    serde_json::to_value(crate::job_store::proxy::evidence::project(job).unwrap()).unwrap()
}

#[test]
fn proxy_evidence_unknown_is_not_release_and_unsigned_retirement_is_explicit() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, policy) = fixture();
    begin(&mut store, "job_unknown", &terms).unwrap();
    let job = store.get("job_unknown", 1).unwrap().unwrap();
    let evidence = public(&job);
    assert_eq!(evidence["schema_version"], 1);
    assert_eq!(evidence["object"], "mayhem.proxy.job_evidence");
    assert_eq!(
        evidence["identity"],
        serde_json::to_value(identity(&terms)).unwrap()
    );
    assert_eq!(evidence["financial"], json!({"kind":"pending"}));
    assert_eq!(evidence["terms"], Value::Null);
    assert_eq!(evidence["result_verified"], false);
    let retired = store
        .fail_proxy_before_authorization("job_unknown", 2)
        .unwrap();
    assert_eq!(
        public(&retired)["financial"],
        json!({"kind":"not_authorized"})
    );
    begin(&mut store, "job_held", &terms).unwrap();
    let held = store
        .authorize_proxy_terms("job_held", terms, policy, 2)
        .unwrap();
    let evidence = public(&held);
    assert_eq!(evidence["phase"], "awaiting_buyer_journal");
    assert_eq!(evidence["financial"], json!({"kind":"pending"}));
    assert_eq!(evidence["acceptance"], Value::Null);
}

#[test]
fn proxy_evidence_model_content_cannot_claim_financial_closure_and_reopen_retains_exact_payment() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, policy) = authorized(&mut store, "job_evidence");
    let mut delivered = output(&terms);
    delivered.response["financial"] = json!({"kind":"canonical", "paid":"0"});
    delivered.response["receipt"] = json!({"au_owed_cum":"0"});
    let job = store
        .retain_proxy_output("job_evidence", delivered, 3)
        .unwrap();
    let evidence = public(&job);
    assert_eq!(evidence["financial"], json!({"kind":"pending"}));
    assert_eq!(evidence["result_verified"], true);
    assert_eq!(
        evidence["settlement_policy"],
        serde_json::to_value(policy).unwrap()
    );
    assert_eq!(evidence["terms"], serde_json::to_value(&terms).unwrap());
    assert!(evidence.get("result").is_none());
    assert!(evidence.get("receipt").is_none());
    assert!(!evidence.to_string().contains("fixture-only response"));
    let marker = stage_paid_closure(&mut store, "job_evidence");
    let pending = public(&store.get("job_evidence", 4).unwrap().unwrap());
    assert_eq!(pending["financial"]["kind"], "canonical");
    assert_eq!(pending["financial"]["outcome"]["kind"], "paid");
    assert_eq!(pending["financial"]["budget_settled"], false);
    let paid = store
        .finish_proxy("job_evidence", marker.clone(), 5)
        .unwrap();
    let final_evidence = public(&paid);
    assert_eq!(final_evidence["financial"]["commitment"], marker.as_str());
    assert_eq!(
        final_evidence["financial"]["observation"],
        serde_json::to_value(proof()).unwrap()
    );
    assert_eq!(final_evidence["financial"]["budget_settled"], true);
    assert!(final_evidence["financial"]["outcome"]["receipt"]["body"]["au_owed_cum"].is_string());
    drop(store);
    let mut store = open(root.path(), 4, 1024 * 1024, 5);
    assert_eq!(
        final_evidence,
        public(&store.get("job_evidence", 5).unwrap().unwrap())
    );
}

#[test]
fn proxy_evidence_local_signing_fence_never_impersonates_canonical_receipt() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, _) = authorized(&mut store, "job_fenced_evidence");
    let fence = ProxyNonAdmission {
        identity: identity(&terms),
        terms: Digest::new(terms.digest().unwrap()).unwrap(),
        maximum: terms.max_spend_au,
        marker: digest(33),
    };
    let pending = store
        .retain_proxy_non_admission("job_fenced_evidence", fence.clone(), 3)
        .unwrap();
    let evidence = public(&pending);
    assert_eq!(evidence["financial"]["kind"], "non_admission");
    assert_eq!(
        evidence["financial"]["maximum_au"],
        terms.max_spend_au.to_string()
    );
    assert_eq!(evidence["financial"]["terms_hash"], terms.digest().unwrap());
    assert_eq!(evidence["financial"]["budget_settled"], false);
    assert!(evidence["financial"].get("observation").is_none());
    assert!(evidence["financial"].get("outcome").is_none());
    let closed = store
        .finish_proxy_non_admission("job_fenced_evidence", &fence.marker, 4)
        .unwrap();
    assert_eq!(public(&closed)["financial"]["budget_settled"], true);
    // The owner can fail before persisting terms. The opaque unsigned fence
    // still proves non-admission for the original identity, without pretending
    // there was accepted authorization or canonical financial settlement.
    begin(&mut store, "job_no_terms", &terms).unwrap();
    store
        .retain_proxy_non_admission("job_no_terms", fence.clone(), 4)
        .unwrap();
    let closed = store
        .finish_proxy_non_admission("job_no_terms", &fence.marker, 5)
        .unwrap();
    let evidence = public(&closed);
    assert_eq!(evidence["terms"], Value::Null);
    assert_eq!(evidence["financial"]["kind"], "non_admission");
    if let Some(directory) = std::env::var_os("MAYHEM_TEST_PROXY_EVIDENCE_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("non_admission_no_terms.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn proxy_evidence_rejects_corrupted_identity_signatures_and_native_substitution() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open(root.path(), 4, 1024 * 1024, 1);
    let (terms, _) = authorized(&mut store, "job_bound_evidence");
    let original = store
        .retain_proxy_output("job_bound_evidence", output(&terms), 3)
        .unwrap();
    for mutation in [
        "model",
        "endpoint",
        "identity",
        "owner",
        "signature",
        "native",
    ] {
        let mut changed = original.clone();
        match mutation {
            "model" => changed.model.push('0'),
            "endpoint" => changed.endpoint_family = "decisions".into(),
            "identity" => changed.proxy.as_mut().unwrap().identity.billing_id = digest(89),
            "owner" => changed.owner_token_id = None,
            "signature" => {
                changed
                    .proxy
                    .as_mut()
                    .unwrap()
                    .authorization
                    .as_mut()
                    .unwrap()
                    .buyer_sig = "0".repeat(128)
            }
            "native" => changed.proxy = None,
            _ => unreachable!(),
        }
        assert!(
            crate::job_store::proxy::evidence::project(&changed).is_err(),
            "{mutation}"
        );
    }
}
