use super::*;
use crate::{
    job_store::{proxy::ProxyPhase, GatewayJobStatus},
    openai::{
        gateway_token_hash, GatewayKeyBudgetLimits, GatewayTokenBudgetPeriod, GatewayTokenRecord,
        GatewayTokenStore,
    },
};
use mayhem_proxy::{
    attempts::Digest, buyer_controller as buyer, financial::negotiation::BuyerNegotiation,
};
use serde_json::Value;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::watch;

pub(crate) mod support;
use support::{digest, private_dir, worker_path, Harness};

struct StoredOwner {
    root: tempfile::TempDir,
    jobs: Arc<Mutex<GatewayJobStore>>,
    access: Arc<GatewayAccessControl>,
    binding: Binding,
    config: GatewayTokenStore,
}
fn limits() -> GatewayKeyBudgetLimits {
    GatewayKeyBudgetLimits {
        max_tokens: 4,
        max_reservations: 16,
    }
}
fn jobs(path: &Path) -> GatewayJobStore {
    GatewayJobStore::durable([71; 32], path.join("jobs"), 4, 1024 * 1024, 60, now_secs()).unwrap()
}
impl StoredOwner {
    fn new(f: &Harness, cap: u128) -> Self {
        let root = private_dir();
        let context = f.context();
        let binding = Binding {
            job_id: "job_proxy_owner".into(),
            token: GatewayTokenAttribution {
                name: "owner-fixture".into(),
                token_id: "token-owner-fixture".into(),
            },
            model: format!(
                "proxy/offer/{}/{}/{}",
                context.offer.market_id,
                context.offer.provider_pubkey,
                context.offer.slot_id().unwrap()
            ),
            fingerprint: digest(91).as_str().into(),
            identity: (&context).into(),
        };
        let config = GatewayTokenStore {
            version: 1,
            tokens: vec![GatewayTokenRecord {
                name: binding.token.name.clone(),
                token_id: binding.token.token_id.clone(),
                token_hash: gateway_token_hash("owner-fixture-key"),
                created_at: 1,
                expires_at: None,
                budget_au: Some(cap),
                budget_period: Some(GatewayTokenBudgetPeriod::Total),
                spent_total_au: 0,
                spent_period_au: 0,
                period_started_at: Some(1),
                max_rate_per_minute: None,
                models: vec![binding.model.clone()],
                last_used_at: None,
                revoked_at: None,
            }],
        };
        let path = root.path().join("tokens.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let access = Arc::new(
            GatewayAccessControl::new(true, config.clone(), Some(path))
                .initialize_durable_key_budget(root.path().join("budget.redb"), limits())
                .unwrap(),
        );
        let mut store = jobs(root.path());
        store
            .begin_proxy(
                binding.job_id.clone(),
                "openai_chat_completions".into(),
                binding.model.clone(),
                Some(binding.token.token_id.clone()),
                binding.fingerprint.clone(),
                binding.identity.clone(),
                now_secs(),
            )
            .unwrap();
        Self {
            root,
            jobs: Arc::new(Mutex::new(store)),
            access,
            binding,
            config,
        }
    }
    fn owner(&self) -> Arc<Owner> {
        Arc::new(Owner::new(
            self.jobs.clone(),
            self.access.clone(),
            self.binding.clone(),
        ))
    }
    fn job(&self) -> StoredGatewayJob {
        self.jobs
            .lock()
            .unwrap()
            .get(&self.binding.job_id, now_secs())
            .unwrap()
            .unwrap()
    }
    fn configure(&mut self, edit: impl FnOnce(&mut GatewayTokenRecord)) {
        edit(&mut self.config.tokens[0]);
        std::fs::write(
            self.root.path().join("tokens.json"),
            serde_json::to_vec(&self.config).unwrap(),
        )
        .unwrap();
    }
    fn reopen(self) -> Self {
        let Self {
            root,
            jobs: old,
            access,
            binding,
            config,
        } = self;
        assert_eq!(
            Arc::strong_count(&old),
            1,
            "all owner handles must be dropped before restart"
        );
        assert_eq!(Arc::strong_count(&access), 1);
        drop(old);
        drop(access);
        let jobs = Arc::new(Mutex::new(jobs(root.path())));
        let access = Arc::new(
            GatewayAccessControl::new(true, config.clone(), Some(root.path().join("tokens.json")))
                .with_durable_key_budget(root.path().join("budget.redb"), limits())
                .unwrap(),
        );
        Self {
            root,
            jobs,
            access,
            binding,
            config,
        }
    }
}

/// Observe the real opaque hook boundary. No test constructs VerifiedOutput or
/// mutates provider receipt/output/closure internals to pass verification.
struct InspectGate {
    owner: Arc<Owner>,
    negotiation: Arc<BuyerNegotiation>,
    recovery: Arc<mayhem_proxy::financial::recovery::BuyerRecovery>,
    reject_after_authorize: AtomicBool,
    reject_after_output: AtomicBool,
    reject_before_fence_budget: AtomicBool,
    outputs: AtomicUsize,
    retained: Mutex<Option<Value>>,
}
impl InspectGate {
    fn new(s: &StoredOwner, f: &Harness) -> Arc<Self> {
        Arc::new(Self {
            owner: s.owner(),
            negotiation: f.negotiation.clone(),
            recovery: f.recovery.clone(),
            reject_after_authorize: AtomicBool::new(false),
            reject_after_output: AtomicBool::new(false),
            reject_before_fence_budget: AtomicBool::new(false),
            outputs: AtomicUsize::new(0),
            retained: Mutex::new(None),
        })
    }
}
impl AuthorizationGate for InspectGate {
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a NonAdmission,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        Box::pin(async move {
            if self.reject_before_fence_budget.load(Ordering::SeqCst) {
                self.owner
                    .jobs
                    .lock()
                    .unwrap()
                    .retain_proxy_non_admission(
                        &self.owner.binding.job_id,
                        ProxyNonAdmission::capture(proof).unwrap(),
                        now_secs(),
                    )
                    .unwrap();
                return Err(GateError::Unavailable);
            }
            self.owner.retain_non_admission(proof).await
        })
    }
    fn authorize<'a>(
        &'a self,
        purchase: &'a PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        Box::pin(async move {
            let id = &self.owner.binding.identity;
            assert!(self
                .negotiation
                .lookup(id.billing_id.clone(), id.billing_attempt)
                .await
                .unwrap()
                .is_none());
            self.owner.authorize(purchase).await?;
            let job = self
                .owner
                .jobs
                .lock()
                .unwrap()
                .get(&self.owner.binding.job_id, now_secs())
                .unwrap()
                .unwrap();
            assert_eq!(
                job.proxy.as_ref().unwrap().phase(),
                ProxyPhase::AwaitingBuyerJournal
            );
            assert_eq!(
                job.proxy.as_ref().unwrap().terms().unwrap(),
                purchase.terms()
            );
            let pending = self.owner.access.pending_key_budgets(None, 64).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].1.maximum, purchase.terms().max_spend_au);
            assert_eq!(pending[0].1.terms, purchase.terms().digest().unwrap());
            assert!(self
                .negotiation
                .lookup(id.billing_id.clone(), id.billing_attempt)
                .await
                .unwrap()
                .is_none());
            if self.reject_after_authorize.load(Ordering::SeqCst) {
                Err(GateError::Unavailable)
            } else {
                Ok(())
            }
        })
    }
    fn retain_verified_output<'a>(
        &'a self,
        output: VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), GateError>> + Send + 'a>> {
        Box::pin(async move {
            let key = Digest::new(output.authorization().terms.digest().unwrap()).unwrap();
            let state = self.recovery.recover(key).await.unwrap();
            assert!(
                !state.outcome_approved && !state.outcome_signed,
                "payment approval cannot precede output persistence"
            );
            let response = output.response().clone();
            self.owner.retain_verified_output(output).await?;
            let job = self
                .owner
                .jobs
                .lock()
                .unwrap()
                .get(&self.owner.binding.job_id, now_secs())
                .unwrap()
                .unwrap();
            assert_eq!(
                job.proxy.as_ref().unwrap().phase(),
                ProxyPhase::OutputRetained
            );
            assert_eq!(job.result.as_ref(), Some(&response));
            let mut first = self.retained.lock().unwrap();
            if let Some(first) = first.as_ref() {
                assert_eq!(
                    first, &response,
                    "recovery must retain the exact original output"
                );
            } else {
                *first = Some(response);
            }
            self.outputs.fetch_add(1, Ordering::SeqCst);
            if self.reject_after_output.load(Ordering::SeqCst) {
                Err(GateError::Unavailable)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn owner_exact_binding_and_budget_intent_survive_without_buyer_journal() {
    let mut f = Harness::start(&worker_path()).await;
    let purchase = f.prepare().await;
    let s = StoredOwner::new(&f, purchase.terms().max_spend_au + 10);
    for field in ["job", "token", "model", "fingerprint", "identity"] {
        let mut wrong = s.binding.clone();
        match field {
            "job" => wrong.job_id.push_str("-foreign"),
            "token" => wrong.token.token_id.push_str("-foreign"),
            "model" => wrong.model.push_str("-foreign"),
            "fingerprint" => wrong.fingerprint = digest(92).as_str().into(),
            _ => wrong.identity.session_id = digest(93),
        }
        assert!(
            Owner::new(s.jobs.clone(), s.access.clone(), wrong)
                .authorize(&purchase)
                .await
                .is_err(),
            "{field}"
        );
        assert_eq!(
            s.job().proxy.unwrap().phase(),
            ProxyPhase::AwaitingAuthorization
        );
        assert!(s.access.pending_key_budgets(None, 64).unwrap().is_empty());
    }
    s.owner().authorize(&purchase).await.unwrap();
    s.owner().authorize(&purchase).await.unwrap();
    let s = s.reopen();
    assert_eq!(
        s.job().proxy.unwrap().phase(),
        ProxyPhase::AwaitingBuyerJournal
    );
    assert_eq!(
        s.jobs.lock().unwrap().pending_proxy(None, 64).unwrap(),
        [s.binding.job_id.clone()]
    );
    assert_eq!(s.access.pending_key_budgets(None, 64).unwrap().len(), 1);
    assert!(s
        .access
        .reserve_budget(&Some(s.binding.token.clone()), "native-over", 11, "buyer")
        .is_err());
    assert!(f
        .negotiation
        .lookup(
            s.binding.identity.billing_id.clone(),
            s.binding.identity.billing_attempt
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.backend_calls(), 0);
    assert_eq!(f.status().await["publications"], 0);
    f.stop().await;
}

#[tokio::test]
async fn owner_budget_rejection_fences_intent_and_cannot_sign_or_dispatch_after_restart() {
    let mut f = Harness::start(&worker_path()).await;
    let maximum = f.prepare().await.terms().max_spend_au;
    let s = StoredOwner::new(&f, maximum - 1);
    let gate = InspectGate::new(&s, &f);
    let (_stop, rx) = watch::channel(false);
    let result = f.buyer.execute(f.request(gate.clone()), rx).await;
    assert!(matches!(
        result.unwrap(),
        buyer::Outcome::NotAdmitted { .. }
    ));
    assert_eq!(s.job().proxy.unwrap().phase(), ProxyPhase::NotAdmitted);
    assert!(s.access.pending_key_budgets(None, 64).unwrap().is_empty());
    assert!(f
        .negotiation
        .lookup(
            s.binding.identity.billing_id.clone(),
            s.binding.identity.billing_attempt
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.backend_calls(), 0);
    assert_eq!(f.status().await["publications"], 0);
    drop(gate);
    let s = s.reopen();
    assert_eq!(s.job().proxy.unwrap().phase(), ProxyPhase::NotAdmitted);
    assert!(s
        .jobs
        .lock()
        .unwrap()
        .pending_proxy(None, 64)
        .unwrap()
        .is_empty());
    // A later balance/cap change cannot turn the original failed intent into a
    // new paid request. Recovery only replays the same permanent signing fence.
    let (_stop, rx) = watch::channel(false);
    let replay = f
        .buyer
        .recover(s.binding.identity.clone(), s.owner(), rx)
        .await
        .unwrap();
    assert!(matches!(replay, buyer::Outcome::NotAdmitted { .. }));
    assert_eq!(f.backend_calls(), 0);
    assert_eq!(f.status().await["publications"], 0);
    f.stop().await;
}

#[tokio::test]
async fn owner_non_admission_recovers_retained_fence_before_budget_release_exactly_once() {
    let mut f = Harness::start(&worker_path()).await;
    let maximum = f.prepare().await.terms().max_spend_au;
    let s = StoredOwner::new(&f, maximum);
    let gate = InspectGate::new(&s, &f);
    gate.reject_after_authorize.store(true, Ordering::SeqCst);
    gate.reject_before_fence_budget
        .store(true, Ordering::SeqCst);
    let (_stop, rx) = watch::channel(false);
    let failure = f
        .buyer
        .execute(f.request(gate.clone()), rx)
        .await
        .err()
        .unwrap();
    assert!(failure.recovery_required);
    assert_eq!(
        s.job().proxy.unwrap().phase(),
        ProxyPhase::NonAdmissionPendingBudget
    );
    assert_eq!(s.access.pending_key_budgets(None, 64).unwrap().len(), 1);
    assert_eq!(f.backend_calls(), 0);
    drop(gate);
    let s = s.reopen();
    for _ in 0..2 {
        let (_stop, rx) = watch::channel(false);
        assert!(matches!(
            f.buyer
                .recover(s.binding.identity.clone(), s.owner(), rx)
                .await
                .unwrap(),
            buyer::Outcome::NotAdmitted { .. }
        ));
    }
    assert_eq!(s.job().proxy.unwrap().phase(), ProxyPhase::NotAdmitted);
    assert_eq!(s.job().status, GatewayJobStatus::Failed);
    assert!(s.access.pending_key_budgets(None, 64).unwrap().is_empty());
    assert_eq!(f.backend_calls(), 0);
    assert_eq!(f.status().await["publications"], 0);
    // The released allowance can fund a different native request, while the
    // original failed proxy request remains fenced rather than re-admitted.
    s.access
        .reserve_budget(
            &Some(s.binding.token.clone()),
            "different-native",
            maximum,
            "buyer",
        )
        .unwrap();
    f.stop().await;
}

#[tokio::test]
async fn owner_output_precedes_approval_then_restart_closes_once_despite_revocation_and_cap() {
    let mut f = Harness::start(&worker_path()).await;
    let mut s = StoredOwner::new(&f, 100_000);
    let gate = InspectGate::new(&s, &f);
    gate.reject_after_output.store(true, Ordering::SeqCst);
    let (stop, rx) = watch::channel(false);
    let failure = f
        .buyer
        .execute(f.request(gate.clone()), rx)
        .await
        .err()
        .expect("lost output-hook acknowledgment");
    assert_eq!(failure.stage, buyer::Stage::Verifying);
    assert!(failure.recovery_required);
    let saved = f
        .negotiation
        .lookup(
            s.binding.identity.billing_id.clone(),
            s.binding.identity.billing_attempt,
        )
        .await
        .unwrap()
        .unwrap();
    let auth = saved.authorization().unwrap();
    gate.owner.note_purchase(saved).await.unwrap();
    let retained = s.job();
    assert_eq!(
        retained.proxy.as_ref().unwrap().phase(),
        ProxyPhase::OutputRetained
    );
    let original = retained.result.unwrap();
    let state = f
        .recovery
        .recover(Digest::new(auth.terms.digest().unwrap()).unwrap())
        .await
        .unwrap();
    assert!(!state.outcome_approved && !state.outcome_signed);
    assert_eq!(
        f.status().await["publications"],
        1,
        "only reservation before owner hook acknowledgment"
    );
    assert!(
        gate.owner
            .close(f.financial.observe(&auth).await.unwrap())
            .await
            .is_err(),
        "open canonical hold cannot close job"
    );
    assert_eq!(s.job().result.as_ref(), Some(&original));
    f.wait_provider_ends(1).await;
    drop(gate);
    s = s.reopen();
    assert_eq!(s.job().result.as_ref(), Some(&original));
    let gate = InspectGate::new(&s, &f);
    let outcome = f
        .buyer
        .recover(s.binding.identity.clone(), gate.clone(), stop.subscribe())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        buyer::Outcome::Completed {
            settlement: FinancialOutcome::Paid { .. },
            ..
        }
    ));
    assert_eq!(s.job().result.as_ref(), Some(&original));
    assert_eq!(
        f.backend_calls(),
        1,
        "recovery must not send another Execute"
    );
    let saved = f
        .negotiation
        .lookup(
            s.binding.identity.billing_id.clone(),
            s.binding.identity.billing_attempt,
        )
        .await
        .unwrap()
        .unwrap();
    gate.owner.note_purchase(saved).await.unwrap();
    // Revocation and lowering the cap prohibit new admission, not settlement of
    // an already signed obligation. The original token remains the accounting owner.
    s.configure(|token| {
        token.revoked_at = Some(now_secs());
        token.budget_au = Some(0);
    });
    // A real boundary failure after canonical outcome persistence must retain
    // both the output and the budget obligation for restart reconciliation.
    std::fs::write(s.root.path().join("tokens.json"), b"{").unwrap();
    assert!(gate
        .owner
        .close(f.financial.observe(&auth).await.unwrap())
        .await
        .is_err());
    assert_eq!(
        s.job().proxy.unwrap().phase(),
        ProxyPhase::ClosurePendingBudget
    );
    assert_eq!(s.job().result.as_ref(), Some(&original));
    assert_eq!(s.access.pending_key_budgets(None, 64).unwrap().len(), 1);
    s.configure(|_| {});
    drop(gate);
    s = s.reopen();
    assert_eq!(
        s.job().proxy.unwrap().phase(),
        ProxyPhase::ClosurePendingBudget
    );
    let owner = s.owner();
    let closed = owner
        .close(f.financial.observe(&auth).await.unwrap())
        .await
        .unwrap();
    assert_eq!(closed.status, GatewayJobStatus::Completed);
    assert_eq!(closed.result.as_ref(), Some(&original));
    let paid = match f
        .financial
        .observe(&auth)
        .await
        .unwrap()
        .financial_outcome()
        .unwrap()
        .unwrap()
    {
        FinancialOutcome::Paid { receipt } => receipt.body.au_owed_cum,
        _ => panic!("canonical paid receipt required"),
    };
    assert_eq!(
        s.access.summary()["tokens"][0]["spent_total_au"],
        paid.to_string()
    );
    assert!(s.access.pending_key_budgets(None, 64).unwrap().is_empty());
    assert!(s
        .jobs
        .lock()
        .unwrap()
        .pending_proxy(None, 64)
        .unwrap()
        .is_empty());
    drop(owner);
    let s = s.reopen();
    assert_eq!(
        s.owner()
            .close(f.financial.observe(&auth).await.unwrap())
            .await
            .unwrap(),
        closed
    );
    assert_eq!(
        s.access.summary()["tokens"][0]["spent_total_au"],
        paid.to_string()
    );
    assert_eq!(f.status().await["publications"], 2);
    f.stop().await;
}
