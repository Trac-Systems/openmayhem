//! An immutable metadata release limits administrative model selection. Recheck
//! exact canonical identity before delegating the original owner's credit hold.
use super::*;
use std::{future::Future, pin::Pin};
pub(super) async fn gate(
    control: Arc<super::super::proxy_control::ProxyControl>,
    request: Arc<proxy_request::Request>,
    inner: Arc<dyn buyer_controller::AuthorizationGate>,
) -> Result<Arc<dyn buyer_controller::AuthorizationGate>, ApiError> {
    if request
        .controls()
        .profile
        .as_ref()
        .is_none_or(|p| p.taxonomy_filters.is_none())
    {
        return Ok(inner);
    }
    let selected = proxy_request::resolve_estimate(control.clone(), request.clone())
        .await
        .map_err(selection_error)?;
    Ok(Arc::new(Gate {
        control,
        request,
        inner,
        selected,
    }))
}
struct Gate {
    control: Arc<super::super::proxy_control::ProxyControl>,
    request: Arc<proxy_request::Request>,
    inner: Arc<dyn buyer_controller::AuthorizationGate>,
    selected: proxy_request::EstimateCandidate,
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
            if terms.offer != self.selected.published.offer
                || terms.endpoint_contract != self.selected.candidate.endpoint_contract.as_str()
                || terms.recipe_hash != self.selected.candidate.recipe_hash.as_str()
                || terms.connection_revision
                    != self.selected.published.membership.connection_revision
            {
                return Err(Rejected);
            }
            let latest =
                proxy_request::resolve_estimate(self.control.clone(), self.request.clone())
                    .await
                    .map_err(|_| Rejected)?;
            if latest.published.offer != self.selected.published.offer
                || latest.published.membership != self.selected.published.membership
                || latest.published.market != self.selected.published.market
                || super::super::now_millis_u64() >= latest.expires_at_ms
            {
                return Err(Rejected);
            }
            self.inner.authorize(purchase).await
        })
    }
}
