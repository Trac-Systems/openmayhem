//! Operator-authorized recovery traffic, separate from purchases and settlement.
//! This owns only durable budget/dispatch/capacity. It neither performs inference
//! nor marks a model healthy. The trusted controller must also obtain a due health
//! recovery observation and validate the exact configured probe before dispatch.
//!
//! An allowance is permission to use the operator's upstream credentials, not a
//! fiat/crypto balance or proof of upstream pricing. Charge the whole declared
//! conservative cost bound at reservation, including cancelled/unknown attempts.
//! Repeated configuration and restarts never replenish that allowance.

use super::*;

const BUDGETS: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_probe_budgets_v1");
const PROBES: TableDefinition<&str, &[u8]> = TableDefinition::new("capacity_probes_v1");
const SCOPES: TableDefinition<&str, &str> = TableDefinition::new("capacity_probe_groups_v1");

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// Cumulative allowance. Increase explicitly to renew; zero disables new probes.
    pub max_attempts: u64,
    pub max_cost_microusd: u64,
    /// Operator-approved upper estimate, not a quote supplied by an untrusted API.
    /// Zero is permitted for an explicitly approved no-charge backend.
    pub per_attempt_cost_microusd: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetStatus {
    pub policy: Budget,
    pub used_attempts: u64,
    pub allocated_cost_microusd: u64,
    /// One bounded diagnostic record, not an inference history or financial receipt.
    pub last_completed: Option<Completion>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub probe: Digest,
    pub evidence: Digest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbePhase {
    Prepared,
    Dispatched,
    /// Read-time interpretation after controller restart, never permission to resend.
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Specification {
    pub route: Digest,
    pub budget_group: Digest,
    pub request_hash: Digest,
    pub connection_digest: Digest,
    pub connection_revision: u64,
    pub recipe_digest: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub id: Digest,
    pub specification: Specification,
    pub controller_fence: u64,
    pub phase: ProbePhase,
    pub allocated_cost_microusd: u64,
}

/// In-process, non-cloneable dispatch authority. A deserialized Probe is not a permit.
pub struct Reservation {
    probe: Probe,
    owner: Digest,
}
impl Reservation {
    pub fn probe(&self) -> &Probe {
        &self.probe
    }
}
pub struct Dispatch {
    probe: Probe,
}
impl Dispatch {
    pub fn probe(&self) -> &Probe {
        &self.probe
    }
}

/// Only trusted validation of terminal inference, confirmed cancellation or proven
/// non-execution may supply this. An evidence hash alone verifies none of those.
pub struct VerifiedCompletion {
    pub probe: Probe,
    pub evidence: Digest,
}

pub(super) fn create_tables(tx: &redb::WriteTransaction) -> Result<()> {
    db(tx.open_table(BUDGETS))?;
    db(tx.open_table(PROBES))?;
    db(tx.open_table(SCOPES))?;
    Ok(())
}
pub(super) fn upgrade(tx: &redb::WriteTransaction, m: &Meta, names: &[String]) -> Result<()> {
    let tables = [BUDGETS.name(), PROBES.name(), SCOPES.name()];
    if m.schema < 5 {
        require(
            tables.iter().all(|t| !names.iter().any(|n| n == t))
                && m.probe_budgets == 0
                && m.probes == 0
                && m.probe_groups == 0,
        )?;
        create_tables(tx)?;
    } else {
        require(tables.iter().all(|t| names.iter().any(|n| n == t)))?;
    }
    require(
        db(db(tx.open_table(BUDGETS))?.len())? == m.probe_budgets
            && db(db(tx.open_table(PROBES))?.len())? == m.probes
            && db(db(tx.open_table(SCOPES))?.len())? == m.probe_groups
            && m.probe_budgets <= m.groups
            && m.probes <= m.probe_groups
            && m.probe_groups <= m.probes.saturating_mul((MAX_CONSTRAINTS + 1) as u64),
    )
}
pub(super) fn has_budget(tx: &redb::WriteTransaction, group: &Digest) -> Result<bool> {
    Ok(db(db(tx.open_table(BUDGETS))?.get(group.as_str()))?.is_some())
}
fn effective(mut probe: Probe, fence: u64) -> Probe {
    if probe.controller_fence != fence && probe.phase == ProbePhase::Dispatched {
        probe.phase = ProbePhase::Uncertain;
    }
    probe
}
fn valid_spec(s: &Specification, route: &RouteState) -> Result<Vec<Digest>> {
    require(
        s.connection_revision > 0 && route.config.id == s.route && route.config.lane == Lane::Proxy,
    )?;
    let groups = group_ids(route)?;
    require(groups.contains(&s.budget_group))?;
    Ok(groups)
}
fn require_index(
    index: &impl ReadableTable<&'static str, &'static str>,
    groups: &[Digest],
    id: &Digest,
) -> Result<()> {
    for group in groups {
        require(db(index.get(group.as_str()))?.is_some_and(|v| v.value() == id.as_str()))?;
    }
    Ok(())
}

impl Authority {
    /// Trusted operator configuration only. This cannot credit a customer wallet,
    /// claim a paid receipt, reset spent allowance or release an outstanding probe.
    pub fn configure_probe_budget(&self, group: &Digest, policy: Budget) -> Result<()> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        let _: Group = read(&db(tx.open_table(GROUPS))?, group.as_str())?;
        let mut budgets = db(tx.open_table(BUDGETS))?;
        let previous = db(budgets.get(group.as_str()))?
            .map(|v| decode::<BudgetStatus>(v.value()))
            .transpose()?;
        if previous.as_ref().is_some_and(|b| b.policy == policy) {
            return Ok(());
        }
        let mut status = if let Some(value) = previous {
            value
        } else {
            m.probe_budgets = m.probe_budgets.checked_add(1).ok_or(Error::Invalid)?;
            require(m.probe_budgets <= m.groups)?;
            BudgetStatus {
                policy: policy.clone(),
                used_attempts: 0,
                allocated_cost_microusd: 0,
                last_completed: None,
            }
        };
        status.policy = policy;
        db(budgets.insert(group.as_str(), encode(&status)?.as_slice()))?;
        save_meta(&tx, &m)?;
        drop(budgets);
        self.commit(tx)
    }
    pub fn probe_budget(&self, group: &Digest) -> Result<Option<BudgetStatus>> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        db(db(tx.open_table(BUDGETS))?.get(group.as_str()))?
            .map(|v| decode(v.value()))
            .transpose()
    }
    /// Separate recovery path: ordinary requests still require full live health.
    /// This ignores only health allowance, never physical/configured ceilings or
    /// active/uncertain work. Obtain a due Monitor::observe_recovery first.
    pub fn reserve_probe(&self, specification: Specification) -> Result<Reservation> {
        let tx = self.write()?;
        let mut m = meta(&tx)?;
        if m.leases.saturating_add(m.probes) >= self.limits.max_leases {
            return Err(Error::Quota);
        }
        let mut routes = db(tx.open_table(ROUTES))?;
        let mut route: RouteState = read(&routes, specification.route.as_str())?;
        let ids = valid_spec(&specification, &route)?;
        let mut scopes = db(tx.open_table(SCOPES))?;
        for group in &ids {
            if db(scopes.get(group.as_str()))?.is_some() {
                return Err(Error::InUse);
            }
        }
        let mut groups = db(tx.open_table(GROUPS))?;
        let mut values = load_groups(&groups, &route)?;
        if self
            .allowances_with_recovery(
                &values,
                &route,
                self.now()?,
                Some(&specification.budget_group),
            )?
            .free
            == 0
        {
            return Err(Error::Busy);
        }
        let mut budgets = db(tx.open_table(BUDGETS))?;
        let mut budget = db(budgets.get(specification.budget_group.as_str()))?
            .map(|v| decode::<BudgetStatus>(v.value()))
            .transpose()?
            .ok_or(Error::ProbeBudget)?;
        let attempts = budget
            .used_attempts
            .checked_add(1)
            .ok_or(Error::ProbeBudget)?;
        let cost = budget
            .allocated_cost_microusd
            .checked_add(budget.policy.per_attempt_cost_microusd)
            .ok_or(Error::ProbeBudget)?;
        if attempts > budget.policy.max_attempts || cost > budget.policy.max_cost_microusd {
            return Err(Error::ProbeBudget);
        }
        let mut entropy = [0u8; 32];
        getrandom::fill(&mut entropy).map_err(|_| Error::Storage)?;
        let id = Digest::hash("mayhem/proxy/recovery-probe/v1", &[&entropy]);
        let mut probes = db(tx.open_table(PROBES))?;
        if db(probes.get(id.as_str()))?.is_some() {
            return Err(Error::Invalid);
        }
        let probe = Probe {
            id,
            specification,
            controller_fence: self.fence,
            phase: ProbePhase::Prepared,
            allocated_cost_microusd: budget.policy.per_attempt_cost_microusd,
        };
        for group in &mut values {
            group.occupied = group.occupied.checked_add(1).ok_or(Error::Invalid)?;
            db(groups.insert(group.id.as_str(), encode(group)?.as_slice()))?;
            db(scopes.insert(group.id.as_str(), probe.id.as_str()))?;
        }
        route.occupied = route.occupied.checked_add(1).ok_or(Error::Invalid)?;
        budget.used_attempts = attempts;
        budget.allocated_cost_microusd = cost;
        m.probes = m.probes.checked_add(1).ok_or(Error::Invalid)?;
        m.probe_groups = m
            .probe_groups
            .checked_add(ids.len() as u64)
            .ok_or(Error::Invalid)?;
        db(routes.insert(route.config.id.as_str(), encode(&route)?.as_slice()))?;
        db(budgets.insert(
            probe.specification.budget_group.as_str(),
            encode(&budget)?.as_slice(),
        ))?;
        db(probes.insert(probe.id.as_str(), encode(&probe)?.as_slice()))?;
        save_meta(&tx, &m)?;
        drop((routes, groups, budgets, scopes, probes));
        self.commit(tx)?;
        Ok(Reservation {
            probe,
            owner: self.nonce.clone(),
        })
    }
    /// Commit before sending. No read/reconnect can recreate this consumable permit.
    pub fn dispatch_probe(&self, reservation: Reservation) -> Result<Dispatch> {
        if reservation.owner != self.nonce || reservation.probe.controller_fence != self.fence {
            return Err(Error::Stale);
        }
        let tx = self.write()?;
        let mut probes = db(tx.open_table(PROBES))?;
        let mut probe: Probe = read(&probes, reservation.probe.id.as_str())?;
        require(probe == reservation.probe && probe.phase == ProbePhase::Prepared)?;
        let route: RouteState = read(
            &db(tx.open_table(ROUTES))?,
            probe.specification.route.as_str(),
        )?;
        let ids = valid_spec(&probe.specification, &route)?;
        require_index(&db(tx.open_table(SCOPES))?, &ids, &probe.id)?;
        let groups = load_groups(&db(tx.open_table(GROUPS))?, &route)?;
        if !self
            .allowances_with_recovery(
                &groups,
                &route,
                self.now()?,
                Some(&probe.specification.budget_group),
            )?
            .fits
        {
            return Err(Error::Busy);
        }
        // A lowered/disabled allowance stops an unsent probe; allocations already
        // dispatched remain accounted regardless of later policy changes.
        let budget: BudgetStatus = read(
            &db(tx.open_table(BUDGETS))?,
            probe.specification.budget_group.as_str(),
        )?;
        if budget.used_attempts > budget.policy.max_attempts
            || budget.allocated_cost_microusd > budget.policy.max_cost_microusd
        {
            return Err(Error::ProbeBudget);
        }
        probe.phase = ProbePhase::Dispatched;
        db(probes.insert(probe.id.as_str(), encode(&probe)?.as_slice()))?;
        drop(probes);
        self.commit(tx)?;
        Ok(Dispatch { probe })
    }
    /// One bounded lookup for any shared scope; unknown probes never expire.
    pub fn probe_for_group(&self, group: &Digest) -> Result<Option<Probe>> {
        self.healthy()?;
        let tx = db(self.database.begin_read())?;
        let scopes = db(tx.open_table(SCOPES))?;
        let Some(id) = db(scopes.get(group.as_str()))? else {
            return Ok(None);
        };
        let probe: Probe = read(&db(tx.open_table(PROBES))?, id.value())?;
        require(probe.id.as_str() == id.value())?;
        let route: RouteState = read(
            &db(tx.open_table(ROUTES))?,
            probe.specification.route.as_str(),
        )?;
        let ids = valid_spec(&probe.specification, &route)?;
        require(ids.contains(group))?;
        require_index(&scopes, &ids, &probe.id)?;
        Ok(Some(effective(probe, self.fence)))
    }
    /// Prepared is durable proof that no dispatch permit was issued. Safe after
    /// restart too; the consumed operator allowance is conservatively not refunded.
    pub fn cancel_prepared_probe(&self, id: &Digest) -> Result<bool> {
        let tx = self.write()?;
        let probes = db(tx.open_table(PROBES))?;
        let Some(value) = db(probes.get(id.as_str()))? else {
            return Ok(false);
        };
        let probe: Probe = decode(value.value())?;
        require(probe.id == *id && probe.phase == ProbePhase::Prepared)?;
        drop(value);
        drop(probes);
        release(
            &tx,
            &probe,
            Digest::hash(
                "mayhem/proxy/unsent-probe/v1",
                &[probe.id.as_str().as_bytes()],
            ),
        )?;
        self.commit(tx)?;
        Ok(true)
    }
    pub fn complete_probe(&self, proof: VerifiedCompletion) -> Result<bool> {
        let tx = self.write()?;
        let probes = db(tx.open_table(PROBES))?;
        let Some(value) = db(probes.get(proof.probe.id.as_str()))? else {
            return Ok(false);
        };
        let probe: Probe = decode(value.value())?;
        require(
            probe.phase == ProbePhase::Dispatched
                && effective(probe.clone(), self.fence) == proof.probe,
        )?;
        drop(value);
        drop(probes);
        release(&tx, &probe, proof.evidence)?;
        self.commit(tx)?;
        Ok(true)
    }
}

fn release(tx: &redb::WriteTransaction, probe: &Probe, evidence: Digest) -> Result<()> {
    let mut m = meta(tx)?;
    let mut groups = db(tx.open_table(GROUPS))?;
    let mut routes = db(tx.open_table(ROUTES))?;
    let mut route: RouteState = read(&routes, probe.specification.route.as_str())?;
    let ids = valid_spec(&probe.specification, &route)?;
    let mut scopes = db(tx.open_table(SCOPES))?;
    require_index(&scopes, &ids, &probe.id)?;
    for mut group in load_groups(&groups, &route)? {
        group.occupied = group.occupied.checked_sub(1).ok_or(Error::Invalid)?;
        db(groups.insert(group.id.as_str(), encode(&group)?.as_slice()))?;
        db(scopes.remove(group.id.as_str()))?;
    }
    route.occupied = route.occupied.checked_sub(1).ok_or(Error::Invalid)?;
    let mut budgets = db(tx.open_table(BUDGETS))?;
    let mut budget: BudgetStatus = read(&budgets, probe.specification.budget_group.as_str())?;
    budget.last_completed = Some(Completion {
        probe: probe.id.clone(),
        evidence,
    });
    db(budgets.insert(
        probe.specification.budget_group.as_str(),
        encode(&budget)?.as_slice(),
    ))?;
    db(routes.insert(route.config.id.as_str(), encode(&route)?.as_slice()))?;
    db(db(tx.open_table(PROBES))?.remove(probe.id.as_str()))?;
    m.probes = m.probes.checked_sub(1).ok_or(Error::Invalid)?;
    m.probe_groups = m
        .probe_groups
        .checked_sub(ids.len() as u64)
        .ok_or(Error::Invalid)?;
    save_meta(tx, &m)
}
