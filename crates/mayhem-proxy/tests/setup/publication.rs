use super::*;
use ed25519_dalek::SigningKey;
use mayhem_proxy::setup::{AdmissionPermit, PublicationReason, PublicationState};
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn signer(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn owned(endpoint: ProxyEndpoint, n: u8) -> Fixture {
    let mut f = Fixture::new(endpoint);
    let key = signer(n);
    let provider = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    f.input.provider_pubkey = Digest::new(provider.clone()).unwrap();
    f.input.market.creator_pubkey = provider.clone();
    let market = f.input.market.id().unwrap();
    f.input.membership.provider_pubkey = provider.clone();
    f.input.membership.market_id = market.clone();
    for offer in &mut f.input.offers {
        offer.provider_pubkey = provider.clone();
        offer.market_id = market.clone();
    }
    f
}
struct Peer {
    child: tokio::process::Child,
    url: String,
    network: Identity,
    client: reqwest::Client,
}
impl Peer {
    async fn start(f: &mut Fixture) -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut child = tokio::process::Command::new("node")
            .arg("intercom/tests/helpers/proxy-setup-publication-fixture.mjs")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(15),
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        let ready: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ready["ready"], true);
        let p = Self {
            child,
            url: ready["url"].as_str().unwrap().into(),
            network: serde_json::from_value(ready["network"].clone()).unwrap(),
            client: reqwest::Client::new(),
        };
        f.input.network = p.network.clone();
        assert_eq!(
            p.call(
                "configure",
                json!({"market":f.input.market,"offers":f.input.offers})
            )
            .await["configured"],
            true
        );
        p
    }
    fn rpc(&self) -> String {
        format!("{}/v1", self.url)
    }
    async fn call(&self, path: &str, body: Value) -> Value {
        let response = self
            .client
            .post(format!("{}/fixture/{path}", self.url))
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {body}");
        body
    }
    async fn permit(
        &self,
        plan: &mayhem_proxy::setup::PublicationPlan,
        rail: &str,
    ) -> AdmissionPermit {
        serde_json::from_value(
            self.call("permit", json!({"intent":plan.operations[0],"rail":rail}))
                .await,
        )
        .unwrap()
    }
    async fn close(mut self) {
        self.child
            .stdin
            .take()
            .unwrap()
            .write_all(b"close\n")
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
fn checked(f: &Fixture) {
    f.store().create(f.input.clone()).unwrap();
    f.store().check(1).unwrap();
}

#[tokio::test]
async fn real_canonical_publication_retains_lost_ack_and_reuses_admission_for_rates_and_second_provider(
) {
    let mut f = owned(ProxyEndpoint::Chat, 121);
    let p = Peer::start(&mut f).await;
    checked(&f);
    let plan = f.store().publication_plan(2, false).unwrap();
    assert_eq!(plan.operations.len(), 2);
    let no_permit = f
        .store()
        .publish(
            2,
            &p.rpc(),
            10000,
            plan.clone().authorize(&signer(121), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_permit.publication_status, "admission_required");
    assert_eq!(p.call("status", json!({})).await["submissions"], 0);
    assert!(matches!(
        f.store().update(no_permit.revision, f.input.clone()),
        Err(Error::PublicationRecovery)
    ));
    let permit = p.permit(&plan, "fiat").await;
    p.call("mode", json!({"hide_after_submit":true})).await;
    let pending = f
        .store()
        .publish(
            no_permit.revision,
            &p.rpc(),
            10000,
            plan.clone()
                .authorize(&signer(121), Some(permit.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        pending.publication.as_ref().unwrap().state,
        PublicationState::Pending
    );
    assert_eq!(
        pending.publication.as_ref().unwrap().confirmed_operations,
        0
    );
    let before = p
        .call("status", json!({"provider":f.input.provider_pubkey}))
        .await;
    assert_eq!(before["provider"]["sequence"], 1);
    assert_eq!(before["submissions"], 1);
    assert_eq!(before["appends"], 1);
    let retained = std::fs::read(f.store.join("draft.json")).unwrap();
    assert!(f
        .store()
        .recover_publication(pending.revision, "http://127.0.0.1:1/v1", 100)
        .await
        .is_err());
    assert_eq!(std::fs::read(f.store.join("draft.json")).unwrap(), retained);
    p.call("mode", json!({})).await;
    let complete = f
        .store()
        .recover_publication(pending.revision, &p.rpc(), 10000)
        .await
        .unwrap();
    assert_eq!(
        complete.publication_status,
        "canonical_operations_confirmed"
    );
    assert_eq!(
        complete.publication.as_ref().unwrap().confirmed_operations,
        2
    );
    assert!(!complete.publication.as_ref().unwrap().authorizes_serving);
    assert_eq!(complete.serving_status, "not_started");
    assert!(complete.admission_handoff.is_none());
    let done = p
        .call("status", json!({"provider":f.input.provider_pubkey}))
        .await;
    assert_eq!(done["provider"]["sequence"], 2);
    assert_eq!(done["appends"], 2);
    assert_eq!(done["submissions"], 2);
    assert_eq!(done["pending"], 0);
    let replay = f
        .store()
        .recover_publication(complete.revision, &p.rpc(), 10000)
        .await
        .unwrap();
    assert_eq!(replay.revision, complete.revision);
    assert_eq!(p.call("status", json!({})).await["submissions"], 2);
    let original_entitlement = done["provider"]["entitlement"].clone();
    let mut rates = f.input.clone();
    rates.sequence = 3;
    rates.offers[0].revision = 2;
    rates.offers[0].rates[0].per_unit_au += 1;
    let changed = f.store().update(complete.revision, rates.clone()).unwrap();
    assert_eq!(changed.publication_status, "previous_publication_confirmed");
    let checked_rates = f.store().check(changed.revision).unwrap();
    let price_plan = f
        .store()
        .publication_plan(checked_rates.revision, true)
        .unwrap();
    assert_eq!(price_plan.operations.len(), 1);
    let priced = f
        .store()
        .publish(
            checked_rates.revision,
            &p.rpc(),
            10000,
            price_plan.authorize(&signer(121), None).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(priced.publication_status, "canonical_operations_confirmed");
    let prices = p
        .call("status", json!({"provider":f.input.provider_pubkey}))
        .await;
    assert_eq!(prices["provider"]["entitlement"], original_entitlement);
    assert_eq!(prices["permits"], 1);
    assert_eq!(prices["appends"], 3);
    let mut second = owned(ProxyEndpoint::Chat, 122);
    second.input.network = p.network.clone();
    second.input.market = f.input.market.clone();
    second.input.selection = Selection::JoinMarket;
    second.input.membership.market_id = f.input.market.id().unwrap();
    second.input.offers[0].market_id = f.input.market.id().unwrap();
    checked(&second);
    let join = second.store().publication_plan(2, false).unwrap();
    let second_permit = p.permit(&join, "tap").await;
    let joined = second
        .store()
        .publish(
            2,
            &p.rpc(),
            10000,
            join.authorize(&signer(122), Some(second_permit)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(joined.publication_status, "canonical_operations_confirmed");
    let final_state = p
        .call("status", json!({"provider":second.input.provider_pubkey}))
        .await;
    assert_eq!(final_state["appends"], 5);
    assert_eq!(final_state["permits"], 2);
    assert_eq!(final_state["provider"]["sequence"], 2);
    assert_eq!(final_state["model_calls"], 0);
    assert_eq!(final_state["financial_collection"], false);
    assert_eq!(
        final_state["native_balance"],
        json!({"fiat":"10","tnk":"20","tap":"30"})
    );
    assert_eq!(
        final_state["native_payout"],
        json!({"status":"prepared","native":true})
    );
    for review in [&complete, &priced, &joined] {
        let public = serde_json::to_string(review).unwrap();
        for hidden in [
            "connection_file",
            "private-upstream-model",
            "never-read-secret",
            "issuer_signature",
            "invoice_commitment",
            "provider_signatures",
            "http://",
        ] {
            assert!(!public.contains(hidden), "{hidden}");
        }
    }
    if let Ok(path) = std::env::var("MAYHEM_TEST_SETUP_PUBLICATION_FIXTURE") {
        private(Path::new(&path),&serde_json::to_vec_pretty(&json!({"schema_version":1,"test_only":true,
            "transport":"real loopback HTTP, signed local Autobase and canonical publication journal; owned-provider read facade",
            "admission":"synthetic verifier-signed permits; no financial collection", "initial":complete,"rates":priced,"second_provider":joined,"counters":final_state})).unwrap());
    }
    f.no_network_or_secret();
    second.no_network_or_secret();
    p.close().await;
}

#[tokio::test]
async fn all_four_endpoint_families_and_three_fee_rails_use_the_real_guarded_writer() {
    for (endpoint, rail, n) in [
        (ProxyEndpoint::Completions, "tnk", 123),
        (ProxyEndpoint::Responses, "tap", 124),
        (ProxyEndpoint::Decisions, "fiat", 125),
    ] {
        let mut f = owned(endpoint, n);
        let p = Peer::start(&mut f).await;
        checked(&f);
        let plan = f.store().publication_plan(2, false).unwrap();
        let permit = p.permit(&plan, rail).await;
        let result = f
            .store()
            .publish(
                2,
                &p.rpc(),
                10000,
                plan.authorize(&signer(n), Some(permit)).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.publication_status, "canonical_operations_confirmed",
            "{rail}"
        );
        let state = p
            .call("status", json!({"provider":f.input.provider_pubkey}))
            .await;
        assert_eq!(state["provider"]["entitlement"]["rail"], rail);
        assert_eq!(state["appends"], 2);
        assert_eq!(state["permits"], 1);
        f.no_network_or_secret();
        p.close().await;
    }
}

#[tokio::test]
async fn changed_signatures_permits_origins_and_canonical_reads_never_publish_or_guess_a_new_sequence(
) {
    let mut f = owned(ProxyEndpoint::Chat, 126);
    let p = Peer::start(&mut f).await;
    checked(&f);
    let plan = f.store().publication_plan(2, false).unwrap();
    let permit = p.permit(&plan, "tnk").await;
    assert!(plan
        .clone()
        .authorize(&signer(127), Some(permit.clone()))
        .is_err());
    let mut wrong = permit.clone();
    wrong.permit.provider_pubkey = d(5).as_str().into();
    assert!(plan.clone().authorize(&signer(126), Some(wrong)).is_err());
    let auth = plan.clone().authorize(&signer(126), Some(permit)).unwrap();
    for url in [
        "http://example.invalid/v1",
        "http://user@127.0.0.1/v1",
        "http://127.0.0.1/v1?x=1",
    ] {
        assert!(f.store().publish(2, url, 100, auth.clone()).await.is_err());
    }
    let mut altered = auth.clone();
    altered.provider_signatures[1] = "0".repeat(128);
    assert!(f.store().publish(2, &p.rpc(), 1000, altered).await.is_err());
    assert_eq!(f.store().inspect().unwrap().revision, 2);
    p.call("mode", json!({"corrupt":true})).await;
    let invalid = f
        .store()
        .publish(2, &p.rpc(), 10000, auth.clone())
        .await
        .unwrap();
    assert_eq!(
        invalid.publication.as_ref().unwrap().reason,
        Some(PublicationReason::InvalidResponse)
    );
    assert_eq!(p.call("status", json!({})).await["appends"], 0);
    let mut config = f.connection.clone();
    config["revision"] = json!(2);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&config).unwrap(),
    );
    p.call("mode", json!({})).await;
    let drift = f
        .store()
        .recover_publication(invalid.revision, &p.rpc(), 10000)
        .await
        .unwrap();
    assert_eq!(
        drift.publication.as_ref().unwrap().reason,
        Some(PublicationReason::ConfigurationChanged)
    );
    assert_eq!(p.call("status", json!({})).await["submissions"], 0);
    private(
        &f.input.connection_file,
        &serde_json::to_vec(&f.connection).unwrap(),
    );
    p.call("mode", json!({"hide_after_submit":true})).await;
    let pending = f
        .store()
        .recover_publication(drift.revision, &p.rpc(), 10000)
        .await
        .unwrap();
    assert_eq!(p.call("status", json!({})).await["appends"], 1);
    // Another owner-authorized publication advanced the provider; recovery must
    // not assume the missing prior ACK or manufacture replacement sequences.
    let op = &auth.plan.operations[1];
    let key = format!(
        "proxy/registry/{}/{}/{}",
        op.provider_pubkey,
        op.sequence,
        op.digest().unwrap()
    );
    let direct=p.client.post(format!("{}/v1/contract/feature",p.url)).json(&json!({"feature":"mayhem","key":key,
        "value":{"op":"proxy_registry","intent":op,"provider_signature":auth.provider_signatures[1],"admission":null}})).send().await.unwrap();
    assert!(direct.status().is_success());
    p.call("mode", json!({})).await;
    let blocked = f
        .store()
        .recover_publication(pending.revision, &p.rpc(), 10000)
        .await
        .unwrap();
    assert_eq!(
        blocked.publication.as_ref().unwrap().reason,
        Some(PublicationReason::SequenceConflict)
    );
    assert_eq!(p.call("status", json!({})).await["appends"], 2);
    assert!(matches!(
        f.store().update(blocked.revision, f.input.clone()),
        Err(Error::PublicationRecovery)
    ));
    f.no_network_or_secret();
    p.close().await;
}
