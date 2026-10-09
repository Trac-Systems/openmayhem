//! Trusted observations checked independently of administrative definitions.
use super::super::proxy_control::ProxyControl;
use super::*;
use mayhem_proxy::{
    conformance::{self, Class, Lookup, Subject},
    directory::PublishedOffer,
    registry::{self, publication::Reference},
    routing::{Policy, Ranking, Target},
};
use std::{future::Future, pin::Pin};
static READS: Semaphore = Semaphore::const_new(4);

pub(crate) fn unsupported(p: &Policy) -> bool {
    p.providers.require_verified_operator
        || !p.constraints.data_handling.is_empty()
        || matches!(&p.target, Target::Category { variants, tags, .. } if !variants.is_empty() || !tags.is_empty())
}
pub(super) fn needed(p: &Policy) -> bool {
    !p.constraints.capabilities.is_empty() || matches!(p.ranking, Ranking::PreferredSpeed)
}
fn unavailable_evidence() -> ApiError {
    selection_error(proxy_request::Error::ProfileEvidence)
}
pub(super) fn subject(p: &PublishedOffer) -> Result<Subject, ApiError> {
    let contract = p
        .membership
        .endpoints
        .iter()
        .find(|e| e.endpoint == p.offer.endpoint)
        .ok_or_else(unavailable_evidence)?;
    Subject::new(
        &p.offer,
        Digest::new(&contract.contract_hash).map_err(|_| unavailable_evidence())?,
        Digest::new(&p.membership.recipe_hash).map_err(|_| unavailable_evidence())?,
        p.membership.connection_revision,
    )
    .map_err(|_| unavailable_evidence())
}
pub(super) async fn check(
    control: Arc<ProxyControl>,
    request: Arc<proxy_request::Request>,
    published: &PublishedOffer,
) -> Result<Option<conformance::Signed>, ApiError> {
    let Some(policy) = &request.controls().profile else {
        return Ok(None);
    };
    if unsupported(policy) {
        return Err(unavailable_evidence());
    }
    if !needed(policy) {
        return Ok(None);
    }
    let store = control
        .conformance()
        .ok_or_else(unavailable_evidence)?
        .clone();
    let subject = subject(published)?;
    let class = Class::request(request.provider_value()).map_err(|_| unavailable_evidence())?;
    let mut definitions = None;
    if !policy.constraints.capabilities.is_empty() {
        let binding = request
            .controls()
            .registry_release
            .as_ref()
            .ok_or_else(unavailable_evidence)?;
        let reader = control.registry().ok_or_else(unavailable_evidence)?;
        let pin = reader
            .pin_release(&binding.release_id)
            .await
            .map_err(|_| unavailable_evidence())?;
        if pin.metadata().release_hash != binding.release_hash.as_str() {
            return Err(unavailable_evidence());
        }
        let refs = policy
            .constraints
            .capabilities
            .iter()
            .map(|p| Reference {
                field_id: p.field_id.clone(),
                schema_revision: p.schema_revision,
            })
            .collect::<Vec<_>>();
        definitions = Some(
            reader
                .resolve_closure(&pin, &refs)
                .await
                .map_err(|_| unavailable_evidence())?,
        );
    }
    let permit = READS.try_acquire().map_err(|_| unavailable_evidence())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let Lookup::Present(record) = store
            .lookup(&subject, &class)
            .map_err(|_| unavailable_evidence())?
        else {
            return Err(unavailable_evidence());
        };
        let policy = request
            .controls()
            .profile
            .as_ref()
            .ok_or_else(unavailable_evidence)?;
        for predicate in &policy.constraints.capabilities {
            let definition = definitions
                .as_ref()
                .and_then(|d| d.get(&predicate.field_id, predicate.schema_revision))
                .ok_or_else(unavailable_evidence)?;
            if store
                .evaluate(definition, predicate, &record)
                .map_err(|_| unavailable_evidence())?
                != registry::Match::Satisfied
            {
                return Err(unavailable_evidence());
            }
        }
        if matches!(policy.ranking, Ranking::PreferredSpeed)
            && (!class.streaming
                || subject.endpoint == ProxyEndpoint::Decisions
                || record.body.provenance != conformance::Provenance::GatewayObservation
                || record.body.tester == subject.provider
                || record.body.speed.is_none())
        {
            return Err(unavailable_evidence());
        }
        Ok(Some(*record))
    })
    .await
    .map_err(|_| unavailable_evidence())?
}

/// Last exact connection/configuration check happens before the owner can create
/// a credit hold or the controller can sign. It is skipped for retained replay.
pub(super) async fn gate(
    control: Arc<ProxyControl>,
    request: Arc<proxy_request::Request>,
    inner: Arc<dyn buyer_controller::AuthorizationGate>,
) -> Result<Arc<dyn buyer_controller::AuthorizationGate>, ApiError> {
    if request
        .controls()
        .profile
        .as_ref()
        .is_none_or(|p| !needed(p))
    {
        return Ok(inner);
    }
    let selected = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    let record = check(control.clone(), request, &selected.published)
        .await?
        .ok_or_else(unavailable_evidence)?;
    Ok(Arc::new(Gate {
        inner,
        store: control
            .conformance()
            .ok_or_else(unavailable_evidence)?
            .clone(),
        record,
    }))
}
struct Gate {
    inner: Arc<dyn buyer_controller::AuthorizationGate>,
    store: Arc<conformance::Store>,
    record: conformance::Signed,
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
            let body = &self.record.body;
            if terms.offer.digest().ok().as_deref() != Some(body.subject.offer_digest.as_str())
                || terms.endpoint_contract != body.subject.endpoint_contract.as_str()
                || terms.recipe_hash != body.subject.recipe_hash.as_str()
                || terms.connection_digest != body.connection_digest.as_str()
                || terms.connection_revision != body.subject.connection_revision
            {
                return Err(Rejected);
            }
            let store = self.store.clone();
            let subject = body.subject.clone();
            let class = body.class.clone();
            let expected = self.record.digest().map_err(|_| Rejected)?;
            let permit = READS.try_acquire().map_err(|_| Rejected)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let Lookup::Present(current) =
                    store.lookup(&subject, &class).map_err(|_| Rejected)?
                else {
                    return Err(Rejected);
                };
                if current.digest().map_err(|_| Rejected)? != expected {
                    return Err(Rejected);
                }
                Ok(())
            })
            .await
            .map_err(|_| Rejected)??;
            self.inner.authorize(purchase).await
        })
    }
}

pub(crate) async fn policy(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let result = (|| {
        if super::super::gateway_bearer_token(&headers)?.is_none() {
            return Err(ApiError::unauthorized(
                "Proxy conformance metadata requires authentication",
                Some("Authorization"),
            ));
        }
        state
            .authorize_existing_gateway_request(&headers, None)?
            .ok_or_else(|| {
                ApiError::unauthorized(
                    "Proxy conformance metadata requires authentication",
                    Some("Authorization"),
                )
            })?;
        let store = state
            .proxy_control()
            .and_then(|c| c.conformance())
            .ok_or_else(unavailable_evidence)?;
        Ok::<_, ApiError>(Json(store.semantics()).into_response())
    })();
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
