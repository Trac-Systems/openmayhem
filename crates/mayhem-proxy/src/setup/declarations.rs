//! Explicit, recoverable provider promises. Setup owns the route and revision;
//! published registry definitions own field meanings. Neither reads nor renewal
//! sign automatically, change financial terms, or install a running config.
use super::*;
use crate::{declaration, registry, signing::Authority};
use serde_json::json;
mod source;
pub use source::DeclarationSource;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationChoice {
    pub field_id: String,
    pub schema_revision: u32,
    pub status: registry::Support,
    pub value: Option<registry::TypedValue>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclarationPlan {
    pub schema_version: u32,
    pub draft_id: Digest,
    pub draft_revision: u64,
    pub registry_release_id: String,
    pub registry_release_hash: Digest,
    pub body: declaration::Body,
    pub plan_digest: Digest,
}
#[derive(Serialize)]
pub struct DeclarationReport {
    pub schema_version: u32,
    pub state: &'static str,
    pub for_current_configuration: bool,
    pub plan: DeclarationPlan,
    pub signed: Option<declaration::Signed>,
    pub assurance: &'static str,
    pub installed_in_runtime: bool,
    pub latest_revision: u64,
    pub observed_by_controller: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retained {
    schema_version: u32,
    plan: DeclarationPlan,
    signed: Option<declaration::Signed>,
}
fn digest(plan: &DeclarationPlan) -> Result<Digest> {
    let mut value = json!(plan);
    value
        .as_object_mut()
        .ok_or(Error::Invalid)?
        .remove("plan_digest");
    let bytes = mayhem_proto::stable_json_bytes(&value).map_err(|_| Error::Invalid)?;
    Ok(Digest::hash(
        "mayhem/proxy/setup-data-handling-plan/v1",
        &[&bytes],
    ))
}
fn subject(record: &Record) -> Result<declaration::Subject> {
    declaration::Subject::new(
        record.input.network.clone(),
        record.input.offers.first().ok_or(Error::Invalid)?,
        &record.input.membership,
    )
    .map_err(|_| Error::Invalid)
}
impl Retained {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && self.plan.schema_version == 1
                && self.plan.draft_revision > 0
                && self.plan.plan_digest == digest(&self.plan)?,
        )?;
        self.plan.body.signing_bytes().map_err(|_| Error::Invalid)?;
        if let Some(signed) = &self.signed {
            signed.verify().map_err(|_| Error::Invalid)?;
            require(
                mayhem_proto::stable_json_bytes(&json!(signed.body)).map_err(|_| Error::Invalid)?
                    == mayhem_proto::stable_json_bytes(&json!(self.plan.body))
                        .map_err(|_| Error::Invalid)?,
            )?;
        }
        Ok(())
    }
    fn report(&self, record: &Record, now: u64) -> Result<DeclarationReport> {
        self.validate()?;
        let current = self.plan.draft_id == record.id
            && self.plan.body.subject == subject(record)?
            && record.input.connection()? == record.connection;
        Ok(DeclarationReport {
            schema_version: 1,
            state: if !current {
                "configuration_changed"
            } else if now < self.plan.body.issued_at_ms {
                "not_yet_valid"
            } else if now >= self.plan.body.expires_at_ms {
                "expired"
            } else if self.signed.is_some() {
                "signed_not_installed"
            } else {
                "needs_confirmation"
            },
            for_current_configuration: current,
            plan: self.plan.clone(),
            signed: self.signed.clone(),
            assurance: "declared_not_verified",
            installed_in_runtime: false,
            latest_revision: self.plan.body.revision,
            observed_by_controller: false,
        })
    }
}
pub(super) fn signed_for_run(
    guard: &store::Guard,
    record: &Record,
    now: u64,
) -> Result<Vec<declaration::Signed>> {
    let Some(retained) = guard.read_json::<Retained>("wizard-declaration-signed.json")? else {
        return Ok(Vec::new());
    };
    let report = retained.report(record, now)?;
    require(report.for_current_configuration && report.state == "signed_not_installed")?;
    Ok(vec![report.signed.ok_or(Error::Invalid)?])
}
impl Store {
    /// Explicit withdrawal keeps the last signed field meanings, sets every
    /// claim to Unknown, and needs the same separate wallet confirmation. It is
    /// available even during a registry outage and cannot change financial terms.
    pub fn plan_declaration_withdrawal(
        &self,
        expected_draft: u64,
        expected_declaration: u64,
        now: u64,
        expires_at_ms: u64,
    ) -> Result<DeclarationReport> {
        require(now < expires_at_ms)?;
        let guard = store::Guard::open(&self.directory)?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        let previous: Retained = guard
            .read_json("wizard-declaration-signed.json")?
            .ok_or(Error::Missing)?;
        previous.validate()?;
        require(previous.plan.draft_id == record.id)?;
        let latest = source::latest_signed(
            &guard,
            &record,
            previous.signed.clone().ok_or(Error::Invalid)?,
        )?;
        if record.revision != expected_draft || latest.body.revision != expected_declaration {
            return Err(Error::Conflict);
        }
        require(
            record.input.connection()? == record.connection
                && latest.body.subject == subject(&record)?,
        )?;
        // If restored source differs from the checkpoint, first explicitly renew
        // from the current registry; do not attach old release metadata to claims.
        require(latest.signature == previous.signed.as_ref().ok_or(Error::Invalid)?.signature)?;
        let mut plan = previous.plan;
        plan.draft_revision = record.revision;
        plan.body.revision = latest.body.revision.checked_add(1).ok_or(Error::Invalid)?;
        plan.body.issued_at_ms = now;
        plan.body.expires_at_ms = expires_at_ms;
        for claim in &mut plan.body.claims {
            claim.status = registry::Support::Unknown;
            claim.value = None;
        }
        plan.plan_digest = digest(&plan)?;
        let retained = Retained {
            schema_version: 1,
            plan,
            signed: None,
        };
        retained.validate()?;
        guard.write_json(
            "wizard-declaration-plan.json",
            "wizard-declaration-plan.next",
            &retained,
        )?;
        retained.report(&record, now)
    }
    pub fn inspect_pending_data_handling(&self, now: u64) -> Result<Option<DeclarationReport>> {
        let guard = store::Guard::open(&self.directory)?;
        let Some(pending) = guard.read_json::<Retained>("wizard-declaration-plan.json")? else {
            return Ok(None);
        };
        pending.validate()?;
        if let Some(signed) = guard.read_json::<Retained>("wizard-declaration-signed.json")? {
            signed.validate()?;
            if signed.plan.plan_digest == pending.plan.plan_digest
                || signed.plan.body.revision >= pending.plan.body.revision
            {
                return Ok(None);
            }
        }
        pending
            .report(&guard.read()?.ok_or(Error::Missing)?, now)
            .map(Some)
    }
    /// The caller resolves these exact definitions through the fixed trusted
    /// registry reader. Browser-supplied definitions cannot construct this type.
    /// A new plan replaces only a pending plan; the last signed original remains
    /// separately retained through edits, failures and lost confirmations.
    pub fn plan_data_handling(
        &self,
        expected_draft_revision: u64,
        expected_declaration_revision: u64,
        definitions: &registry::publication::Definitions,
        choices: Vec<DeclarationChoice>,
        now: u64,
        expires_at_ms: u64,
    ) -> Result<DeclarationReport> {
        require(
            !choices.is_empty()
                && choices.len() <= 32
                && choices.windows(2).all(|c| c[0].field_id < c[1].field_id)
                && now < expires_at_ms,
        )?;
        let guard = store::Guard::open(&self.directory)?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        if record.revision != expected_draft_revision {
            return Err(Error::Conflict);
        }
        if record.input.connection()? != record.connection {
            return Err(Error::ConnectionChanged);
        }
        let previous: Option<Retained> = guard.read_json("wizard-declaration-signed.json")?;
        let revision = if let Some(previous) = &previous {
            previous.validate()?;
            require(previous.plan.draft_id == record.id && previous.signed.is_some())?;
            source::latest_signed(&guard, &record, previous.signed.clone().ok_or(Error::Invalid)?)?;
            previous.plan.body.revision
        } else {
            0
        };
        let revision = source::authoring_revision(&guard, &record, revision)?;
        if revision != expected_declaration_revision {
            return Err(Error::Conflict);
        }
        let subject = subject(&record)?;
        let mut claims = Vec::with_capacity(choices.len());
        for choice in choices {
            let definition = definitions
                .get(&choice.field_id, choice.schema_revision)
                .ok_or(Error::Invalid)?;
            require(
                matches!(definition.usage, registry::Usage::FilterOnly)
                    && definition.endpoints.contains(&subject.endpoint)
                    && (choice.status == registry::Support::Supported) == choice.value.is_some(),
            )?;
            if let Some(value) = &choice.value {
                definition
                    .value_schema
                    .accepts(value)
                    .map_err(|_| Error::Invalid)?;
            }
            claims.push(declaration::Claim {
                field_id: choice.field_id,
                schema_revision: choice.schema_revision,
                definition_digest: Digest::new(definition.digest().map_err(|_| Error::Invalid)?)
                    .map_err(|_| Error::Invalid)?,
                status: choice.status,
                value: choice.value,
            });
        }
        let metadata = definitions.release().metadata();
        let mut plan = DeclarationPlan {
            schema_version: 1,
            draft_id: record.id.clone(),
            draft_revision: record.revision,
            registry_release_id: metadata.release_id.clone(),
            registry_release_hash: Digest::new(metadata.release_hash.clone())
                .map_err(|_| Error::Invalid)?,
            body: declaration::Body {
                schema_version: 1,
                subject,
                revision: revision.checked_add(1).ok_or(Error::Invalid)?,
                issued_at_ms: now,
                expires_at_ms,
                claims,
            },
            plan_digest: Digest::hash("placeholder", &[]),
        };
        plan.plan_digest = digest(&plan)?;
        let retained = Retained {
            schema_version: 1,
            plan,
            signed: None,
        };
        retained.validate()?;
        guard.write_json(
            "wizard-declaration-plan.json",
            "wizard-declaration-plan.next",
            &retained,
        )?;
        retained.report(&record, now)
    }
    /// Explicit confirmation signs only the persisted reviewed body, through the
    /// existing role/network-bound wallet authority. Repeated confirmation returns
    /// the exact signed original, even after expiry; it never refreshes its time.
    pub fn confirm_data_handling(
        &self,
        expected_draft_revision: u64,
        expected_plan: &Digest,
        authority: &Authority,
        now: u64,
    ) -> Result<DeclarationReport> {
        let guard = store::Guard::open(&self.directory)?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        let previous: Option<Retained> = guard.read_json("wizard-declaration-signed.json")?;
        if let Some(previous) = &previous {
            previous.validate()?;
            require(previous.plan.draft_id == record.id && previous.signed.is_some())?;
            source::latest_signed(&guard, &record, previous.signed.clone().ok_or(Error::Invalid)?)?;
            if &previous.plan.plan_digest == expected_plan
                && source::authoring_revision(&guard, &record, previous.plan.body.revision)?
                    == previous.plan.body.revision
            {
                return previous.report(&record, now);
            }
        }
        let mut retained: Retained = guard
            .read_json("wizard-declaration-plan.json")?
            .ok_or(Error::Missing)?;
        retained.validate()?;
        if record.input.connection()? != record.connection {
            return Err(Error::ConnectionChanged);
        }
        let previous_revision = source::authoring_revision(
            &guard,
            &record,
            previous.as_ref().map(|p| p.plan.body.revision).unwrap_or(0),
        )?;
        if record.revision != expected_draft_revision
            || retained.plan.draft_revision != record.revision
            || &retained.plan.plan_digest != expected_plan
            || retained.plan.draft_id != record.id
            || retained.plan.body.subject != subject(&record)?
            || retained.plan.body.revision
                != previous_revision.checked_add(1).ok_or(Error::Invalid)?
        {
            return Err(Error::Conflict);
        }
        require(retained.plan.body.issued_at_ms <= now && now < retained.plan.body.expires_at_ms)?;
        retained.signed = Some(
            authority
                .declare_data_handling(retained.plan.body.clone())
                .map_err(|_| Error::Invalid)?,
        );
        guard.write_json(
            "wizard-declaration-signed.json",
            "wizard-declaration-signed.next",
            &retained,
        )?;
        retained.report(&record, now)
    }
    /// Retained public promises only. No upstream call, signer, renewal, model
    /// restart, ledger write or financial mutation is reachable from inspection.
    pub fn inspect_data_handling(&self, now: u64) -> Result<Option<DeclarationReport>> {
        let guard = store::Guard::open(&self.directory)?;
        let Some(retained) = guard.read_json::<Retained>("wizard-declaration-signed.json")? else {
            return Ok(None);
        };
        require(retained.signed.is_some())?;
        let record = guard.read()?.ok_or(Error::Missing)?;
        let mut report = retained.report(&record, now)?;
        if let Some(signed) = &report.signed {
            report.observed_by_controller = source::observed(&guard, &record, signed, now)?;
        }
        report.latest_revision =
            source::authoring_revision(&guard, &record, report.plan.body.revision)?;
        if report.latest_revision > report.plan.body.revision {
            report.state = "superseded_requires_explicit_renewal";
        }
        Ok(Some(report))
    }
}
