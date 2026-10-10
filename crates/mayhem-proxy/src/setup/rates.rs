//! Explicit commercial-only authoring. Retained review is not publication or
//! fresh canonical authority. The existing publisher owns signed recovery.
use super::*;
use crate::financial::{offer::Query, Client};
use ed25519_dalek::SigningKey;
use mayhem_proto::proxy::{ProxyRate, PROXY_MAX_SAFE_INTEGER};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateChoice {
    pub slot_id: Digest,
    pub rates: Vec<ProxyRate>,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub per_request_au: u128,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub min_session_au: u128,
}
impl RateChoice {
    pub fn from_offer(offer: &ProxyOffer) -> Result<Self> {
        Ok(Self {
            slot_id: Digest::new(offer.slot_id().map_err(|_| Error::Invalid)?)
                .map_err(|_| Error::Invalid)?,
            rates: offer.rates.clone(),
            per_request_au: offer.per_request_au,
            min_session_au: offer.min_session_au,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatePlan {
    pub schema_version: u32,
    pub draft_id: Digest,
    pub draft_revision: u64,
    pub previous_binding: Digest,
    pub next_binding: Digest,
    pub previous_offers: Vec<ProxyOffer>,
    pub publication: PublicationPlan,
    pub plan_digest: Digest,
}
#[derive(Serialize)]
pub struct RateReport {
    pub schema_version: u32,
    pub state: &'static str,
    pub plan: RatePlan,
    pub requires_fresh_canonical_check: bool,
    pub changes_execution: bool,
    pub collects_admission_fee: bool,
}
fn digest(plan: &RatePlan) -> Result<Digest> {
    let mut value = json!(plan);
    value
        .as_object_mut()
        .ok_or(Error::Invalid)?
        .remove("plan_digest");
    let bytes = mayhem_proto::stable_json_bytes(&value).map_err(|_| Error::Invalid)?;
    Ok(Digest::hash("mayhem/proxy/setup-rate-plan/v1", &[&bytes]))
}
fn units(offer: &ProxyOffer) -> BTreeSet<&str> {
    offer.rates.iter().map(|r| r.unit.as_str()).collect()
}
fn same_execution(previous: &ProxyOffer, next: &ProxyOffer) -> bool {
    let mut permitted = previous.clone();
    permitted.revision = next.revision;
    permitted.rates = next.rates.clone();
    permitted.per_request_au = next.per_request_au;
    permitted.min_session_au = next.min_session_au;
    permitted == *next && units(previous) == units(next)
}
impl RatePlan {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.draft_revision > 0
                && self.draft_revision < PROXY_MAX_SAFE_INTEGER
                && self.draft_id == self.publication.draft_id
                && self.plan_digest == digest(self)?
                && (1..=16).contains(&self.previous_offers.len())
                && self.previous_offers.len() == self.publication.operations.len()
                && serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() <= MAX_BYTES,
        )?;
        let first = self.publication.operations.first().ok_or(Error::Invalid)?;
        for (i, (old, op)) in self
            .previous_offers
            .iter()
            .zip(&self.publication.operations)
            .enumerate()
        {
            old.validate().map_err(|_| Error::Invalid)?;
            op.validate().map_err(|_| Error::Invalid)?;
            let ProxyAction::SetOffer { offer } = &op.action else {
                return Err(Error::Invalid);
            };
            require(
                first.sequence.checked_add(i as u64) == Some(op.sequence)
                    && old.revision.checked_add(1) == Some(offer.revision)
                    && same_execution(old, offer),
            )?;
        }
        Ok(())
    }
    fn input(&self, record: &Record) -> Result<Input> {
        self.validate()?;
        require(self.draft_id == record.id)?;
        let mut input = record.input.clone();
        input.sequence = self.publication.operations[0].sequence;
        input.offers = self
            .publication
            .operations
            .iter()
            .map(|op| match &op.action {
                ProxyAction::SetOffer { offer } => Ok(offer.clone()),
                _ => Err(Error::Invalid),
            })
            .collect::<Result<_>>()?;
        require(
            input.offers.len() == record.input.offers.len()
                && record
                    .input
                    .offers
                    .iter()
                    .zip(&input.offers)
                    .all(|(old, next)| same_execution(old, next)),
        )?;
        input.validate()?;
        Ok(input)
    }
    fn report(self, record: &Record) -> Result<RateReport> {
        self.validate()?;
        let binding = record.binding()?;
        let current = self.draft_id == record.id && record.input.connection()? == record.connection;
        let published = record
            .publication
            .as_ref()
            .map(|p| p.report(record))
            .transpose()?;
        let state = if !current {
            "configuration_changed"
        } else if binding == self.next_binding {
            if published.as_ref().is_some_and(|p| {
                p.plan_digest == self.publication.plan_digest && p.for_current_configuration
            }) {
                if published
                    .as_ref()
                    .is_some_and(|p| p.state == PublicationState::Complete)
                {
                    "canonical_rates_confirmed"
                } else {
                    "recover_original_publication"
                }
            } else {
                "ready_to_resume_publication"
            }
        } else if record.revision == self.draft_revision && binding == self.previous_binding {
            "needs_confirmation"
        } else {
            "configuration_changed"
        };
        Ok(RateReport {
            schema_version: 1,
            state,
            plan: self,
            requires_fresh_canonical_check: true,
            changes_execution: false,
            collects_admission_fee: false,
        })
    }
}

// One bounded pass, using the existing authenticated exact-key services. A
// changed provider sequence between the surrounding reads invalidates the pass.
async fn current(record: &Record, peer: &str, timeout_ms: u64) -> Result<(u64, Vec<ProxyOffer>)> {
    require((1..=10_000).contains(&timeout_ms))?;
    let (http, base) = admission::peer(peer, timeout_ms)?;
    let client = Client::new(
        peer,
        record.input.network.clone(),
        record.input.provider_pubkey.as_str().into(),
        1,
    )
    .map_err(|_| Error::Invalid)?;
    let operation = Digest::new(
        record
            .input
            .operation()
            .digest()
            .map_err(|_| Error::Invalid)?,
    )
    .map_err(|_| Error::Invalid)?;
    let observe = || async {
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| Error::Storage)?;
        admission::observe(
            &http,
            &base,
            &record.input.network,
            &record.input.provider_pubkey,
            &operation,
            &Digest::hash("mayhem/proxy/setup-rates-read/v1", &[&random]),
        )
        .await
        .map_err(|_| Error::RatesUnavailable)
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let first = observe().await?;
        require(first.registry_enabled && !first.provider_revoked && !first.admission_revoked)?;
        let provider = first.provider.as_ref().ok_or(Error::Invalid)?;
        let previous = record
            .publication
            .as_ref()
            .ok_or(Error::Invalid)?
            .report(record)?;
        if let Some(e) = previous.canonical_observation {
            require(first.proof.follows(&e.proof) && first.context.epoch >= e.context.epoch)?;
        }
        if let Some((proof, epoch)) = record.admission.as_ref().and_then(|a| a.high_water()) {
            require(first.proof.follows(&proof) && first.context.epoch >= epoch)?;
        }
        let next = provider
            .sequence
            .checked_add(1)
            .filter(|v| *v <= PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Invalid)?;
        require(
            next.checked_add(record.input.offers.len() as u64 - 1)
                .is_some_and(|v| v <= PROXY_MAX_SAFE_INTEGER),
        )?;
        let mut proof = first.proof.clone();
        let mut offers = Vec::new();
        for offer in &record.input.offers {
            let query = Query {
                offer: offer.clone(),
                rail: *offer.accepted_rails.first().ok_or(Error::Invalid)?,
                settlement_policy_hash: record
                    .input
                    .settlement_policy
                    .digest()
                    .map_err(|_| Error::Invalid)?,
            };
            let observed = client
                .current_rate_state(&query)
                .await
                .map_err(|_| Error::RatesUnavailable)?;
            let latest = observed.offer().map_err(|_| Error::Invalid)?;
            require(
                observed.proof().follows(&proof)
                    && observed.membership().map_err(|_| Error::Invalid)?
                        == &record.input.membership
                    && observed.policy().map_err(|_| Error::Invalid)?
                        == &record.input.settlement_policy
                    && latest.revision >= offer.revision
                    && same_execution(offer, latest),
            )?;
            proof = observed.proof().clone();
            offers.push(latest.clone());
        }
        let last = observe().await?;
        require(
            last.registry_enabled
                && !last.provider_revoked
                && !last.admission_revoked
                && last.proof.follows(&proof)
                && last.context.epoch >= first.context.epoch
                && last.provider.as_ref().is_some_and(|p| {
                    p.sequence == provider.sequence
                        && p.operation_digest == provider.operation_digest
                        && p.entitlement_id == provider.entitlement_id
                }),
        )?;
        Ok((next, offers))
    })
    .await
    .map_err(|_| Error::RatesUnavailable)?
}
impl Store {
    /// Read-only inspection of one retained proposal; never refreshes authority.
    pub fn rates(&self) -> Result<Option<RateReport>> {
        let guard = store::Guard::open(&self.directory)?;
        let Some(record) = guard.read()? else {
            return Ok(None);
        };
        guard
            .read_json::<RatePlan>("wizard-rates.json")?
            .map(|p| p.report(&record))
            .transpose()
    }
    /// Reads canonical facts and retains a commercial-only review. No draft
    /// change, wallet access, inference, admission invoice or signed submission.
    pub async fn plan_rates(
        &self,
        expected_revision: u64,
        choices: Vec<RateChoice>,
        peer: &str,
        timeout_ms: u64,
    ) -> Result<RateReport> {
        let guard = store::Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected_revision {
            return Err(Error::Conflict);
        }
        let publication = record
            .publication
            .as_ref()
            .ok_or(Error::Invalid)?
            .report(&record)?;
        require(
            publication.state == PublicationState::Complete
                && publication.for_current_configuration
                && record.checked.as_ref() == Some(&record.binding()?)
                && choices.len() == record.input.offers.len()
                && serde_json::to_vec(&choices)
                    .map_err(|_| Error::Invalid)?
                    .len()
                    <= 64 * 1024,
        )?;
        let (sequence, previous_offers) = current(&record, peer, timeout_ms).await?;
        let previous_binding = record.binding()?;
        let original_input = record.input.clone();
        record.input.sequence = sequence;
        let mut used = BTreeSet::new();
        for (target, previous) in record.input.offers.iter_mut().zip(&previous_offers) {
            let slot = previous.slot_id().map_err(|_| Error::Invalid)?;
            let choice = choices
                .iter()
                .find(|c| c.slot_id.as_str() == slot)
                .ok_or(Error::Invalid)?;
            require(used.insert(choice.slot_id.clone()))?;
            *target = previous.clone();
            target.revision = previous
                .revision
                .checked_add(1)
                .filter(|v| *v <= PROXY_MAX_SAFE_INTEGER)
                .ok_or(Error::Invalid)?;
            target.rates = choice.rates.clone();
            target.per_request_au = choice.per_request_au;
            target.min_session_au = choice.min_session_au;
            require(same_execution(previous, target))?;
        }
        require(
            record
                .input
                .offers
                .iter()
                .zip(&previous_offers)
                .any(|(next, old)| {
                    next.rates != old.rates
                        || next.per_request_au != old.per_request_au
                        || next.min_session_au != old.min_session_au
                }),
        )?;
        record.input.validate()?;
        let next_binding = record.binding()?;
        record.checked = Some(next_binding.clone());
        let publication = record.publication_plan(true)?;
        let mut plan = RatePlan {
            schema_version: 1,
            draft_id: record.id.clone(),
            draft_revision: record.revision,
            previous_binding,
            next_binding,
            previous_offers,
            publication,
            plan_digest: Digest::hash("pending", &[]),
        };
        plan.plan_digest = digest(&plan)?;
        plan.validate()?;
        record.input = original_input;
        record.checked = Some(record.binding()?);
        guard.write_json("wizard-rates.json", "wizard-rates.next", &plan)?;
        plan.report(&record)
    }
    /// Confirmation rechecks original canonical inputs before signing. Once
    /// retained by the publisher, the exact original operations recover first.
    pub async fn publish_rates(
        &self,
        expected_revision: u64,
        plan_digest: &Digest,
        peer: &str,
        timeout_ms: u64,
        key: &SigningKey,
    ) -> Result<Review> {
        let guard = store::Guard::open(&self.directory)?;
        let mut record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected_revision {
            return Err(Error::Conflict);
        }
        let plan = guard
            .read_json::<RatePlan>("wizard-rates.json")?
            .ok_or(Error::Missing)?;
        plan.validate()?;
        require(
            &plan.plan_digest == plan_digest
                && plan.draft_id == record.id
                && record.input.connection()? == record.connection,
        )?;
        let binding = record.binding()?;
        let published = record
            .publication
            .as_ref()
            .ok_or(Error::Invalid)?
            .report(&record)?;
        let recovering =
            binding == plan.next_binding && published.plan_digest == plan.publication.plan_digest;
        if !recovering {
            require(
                (record.revision == plan.draft_revision && binding == plan.previous_binding)
                    || (record.revision == plan.draft_revision + 1 && binding == plan.next_binding),
            )?;
            require(published.state == PublicationState::Complete)?;
            // If the draft was saved but the publisher was not yet entered,
            // read using the old offers, not unpublished proposed revisions.
            let saved = record.input.clone();
            record.input.offers = plan.previous_offers.clone();
            let (sequence, offers) = current(&record, peer, timeout_ms).await?;
            record.input = saved;
            require(
                sequence == plan.publication.operations[0].sequence
                    && offers == plan.previous_offers,
            )
            .map_err(|_| Error::RatesChanged)?;
        }
        let authorization = plan.publication.clone().authorize(key, None)?;
        if binding == plan.previous_binding {
            record.retain_probe_configuration()?;
            record.input = plan.input(&record)?;
            require(record.binding()? == plan.next_binding)?;
            record.checked = Some(plan.next_binding.clone());
            record.next(expected_revision)?;
            guard.write(&record)?;
        } else {
            require(binding == plan.next_binding)?;
        }
        let revision = record.revision;
        drop(guard);
        self.publish(revision, peer, timeout_ms, authorization)
            .await
    }
}
