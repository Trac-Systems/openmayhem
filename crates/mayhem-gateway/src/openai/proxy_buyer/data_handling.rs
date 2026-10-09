//! Provider declarations satisfy only explicit Declared requirements. Neither
//! operator identity nor a successful protocol probe upgrades these promises.
use super::*;
use mayhem_proxy::{
    declaration,
    directory::PublishedOffer,
    registry::{self, publication::Reference},
};
use std::{future::Future, pin::Pin};
static READS: Semaphore = Semaphore::const_new(mayhem_proxy::descriptor::READS);

pub(super) fn needed(request: &proxy_request::Request) -> bool {
    request
        .controls()
        .profile
        .as_ref()
        .is_some_and(|p| !p.constraints.data_handling.is_empty())
}
pub(super) enum Failure {
    Unavailable,
    Unsatisfied,
}
impl Failure {
    pub(super) fn api(self) -> ApiError {
        selection_error(match self {
            Self::Unavailable => proxy_request::Error::ProfileEvidence,
            Self::Unsatisfied => proxy_request::Error::Constraints,
        })
    }
}
#[derive(Clone)]
pub(super) struct Observation {
    pub record: declaration::Signed,
    pub expires_at_ms: u64,
}
pub(super) async fn check(
    control: Arc<super::super::proxy_control::ProxyControl>,
    controller: &buyer_controller::Controller,
    request: Arc<proxy_request::Request>,
    published: &PublishedOffer,
) -> Result<Option<Observation>, Failure> {
    use Failure::Unavailable as Bad;
    if !needed(&request) {
        return Ok(None);
    }
    let _permit = READS.try_acquire().map_err(|_| Bad)?;
    let policy = request.controls().profile.as_ref().ok_or(Bad)?;
    let binding = request.controls().registry_release.as_ref().ok_or(Bad)?;
    let reader = control.registry().ok_or(Bad)?;
    let pin = reader
        .pin_release(&binding.release_id)
        .await
        .map_err(|_| Bad)?;
    if pin.metadata().release_hash != binding.release_hash.as_str() {
        return Err(Bad);
    }
    let refs = policy
        .constraints
        .data_handling
        .iter()
        .map(|p| Reference {
            field_id: p.field_id.clone(),
            schema_revision: p.schema_revision,
        })
        .collect::<Vec<_>>();
    let definitions = reader.resolve_closure(&pin, &refs).await.map_err(|_| Bad)?;
    let identity = controller.identity();
    let network = mayhem_proxy::discovery::Identity {
        network_id: identity.network_id.clone(),
        msb_bootstrap: identity.msb_bootstrap.as_str().into(),
        subnet_bootstrap: identity.subnet_bootstrap.as_str().into(),
        contract_version: mayhem_proto::CONTRACT_VERSION,
    };
    let subject = declaration::Subject::new(network, &published.offer, &published.membership)
        .map_err(|_| Bad)?;
    let (_, signed) = controller
        .describe_with_declaration(
            published.offer.clone(),
            request.controls().rail,
            request.controls().settlement_policy_hash.clone(),
            &subject.endpoint_contract,
            &subject.recipe_hash,
        )
        .await
        .map_err(|_| Bad)?;
    let record = signed.ok_or(Bad)?;
    let now = super::super::now_millis_u64();
    record.check(&subject, now).map_err(|_| Bad)?;
    for predicate in &policy.constraints.data_handling {
        let definition = definitions
            .get(&predicate.field_id, predicate.schema_revision)
            .ok_or(Bad)?;
        match record
            .evaluate_with_rules(
                definition,
                predicate,
                |id, revision| definitions.get(id, revision),
                now,
            )
            .map_err(|_| Bad)?
        {
            registry::Match::Satisfied => {}
            registry::Match::DifferentValue | registry::Match::Unsupported => {
                return Err(Failure::Unsatisfied)
            }
            _ => return Err(Bad),
        }
    }
    let mut expires_at_ms = record
        .body
        .expires_at_ms
        .min(now.saturating_add(declaration::MAX_READ_AGE_MS));
    // Quote validity cannot outlive any retained definition/requirement's
    // freshness bound, including the bounded conditional-definition closure.
    for age in policy
        .constraints
        .data_handling
        .iter()
        .filter_map(|p| p.max_age_ms)
        .chain(
            definitions
                .documents()
                .filter_map(|d| d.definition.max_evidence_age_ms),
        )
    {
        expires_at_ms = expires_at_ms.min(
            record
                .body
                .issued_at_ms
                .saturating_add(u64::from(age))
                .saturating_add(1),
        );
    }
    if expires_at_ms <= now {
        return Err(Bad);
    }
    Ok(Some(Observation {
        record,
        expires_at_ms,
    }))
}

pub(super) async fn gate(
    control: Arc<super::super::proxy_control::ProxyControl>,
    controller: Arc<buyer_controller::Controller>,
    request: Arc<proxy_request::Request>,
    inner: Arc<dyn buyer_controller::AuthorizationGate>,
) -> Result<Arc<dyn buyer_controller::AuthorizationGate>, ApiError> {
    if !needed(&request) {
        return Ok(inner);
    }
    let selected = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    let observation = check(
        control.clone(),
        &controller,
        request.clone(),
        &selected.published,
    )
    .await
    .map_err(Failure::api)?
    .ok_or_else(|| Failure::Unavailable.api())?;
    Ok(Arc::new(Gate {
        control,
        controller,
        request,
        inner,
        observation,
        offer: selected.published.offer,
    }))
}
struct Gate {
    control: Arc<super::super::proxy_control::ProxyControl>,
    controller: Arc<buyer_controller::Controller>,
    request: Arc<proxy_request::Request>,
    inner: Arc<dyn buyer_controller::AuthorizationGate>,
    observation: Observation,
    offer: mayhem_proto::proxy::ProxyOffer,
}
impl buyer_controller::AuthorizationGate for Gate {
    fn retain_non_admission<'a>(
        &'a self,
        proof: &'a mayhem_proxy::financial::negotiation::NonAdmission,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer_controller::GateError>> + Send + 'a>> {
        self.inner.retain_non_admission(proof)
    }
    fn retain_verified_output<'a>(
        &'a self,
        output: buyer_controller::VerifiedOutput<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer_controller::GateError>> + Send + 'a>> {
        self.inner.retain_verified_output(output)
    }
    fn authorize<'a>(
        &'a self,
        purchase: &'a mayhem_proxy::financial::quote::PreparedPurchase,
    ) -> Pin<Box<dyn Future<Output = Result<(), buyer_controller::GateError>> + Send + 'a>> {
        Box::pin(async move {
            use buyer_controller::GateError::Rejected;
            let terms = purchase.terms();
            let subject = &self.observation.record.body.subject;
            if super::super::now_millis_u64() >= self.observation.expires_at_ms
                || terms.offer != self.offer
                || terms.endpoint_contract != subject.endpoint_contract.as_str()
                || terms.recipe_hash != subject.recipe_hash.as_str()
                || terms.connection_revision != subject.connection_revision
            {
                return Err(Rejected);
            }
            let latest =
                proxy_request::resolve_estimate(self.control.clone(), self.request.clone())
                    .await
                    .map_err(|_| Rejected)?;
            if latest.published.offer != self.offer {
                return Err(Rejected);
            }
            let current = check(
                self.control.clone(),
                &self.controller,
                self.request.clone(),
                &latest.published,
            )
            .await
            .map_err(|_| Rejected)?
            .ok_or(Rejected)?;
            if current.record.digest().map_err(|_| Rejected)?
                != self.observation.record.digest().map_err(|_| Rejected)?
            {
                return Err(Rejected);
            }
            self.inner.authorize(purchase).await
        })
    }
}
