//! Indexed canonical candidate enumeration, not routing or a cost prediction.
//! Each page has bounded work; callers may continue to exhaust any size scope.
use super::*;
use crate::routing::{Policy, Target};
use mayhem_proto::proxy::{ProxyMembership, ProxyRate, PROXY_MAX_SAFE_INTEGER};
use serde_json::Value;

const ROOT: &str = "candidate/v1/";
const PROBE_ROWS: usize = 8;
/// Includes driver probes, exhausted ranges, nested joins and returned rows.
pub const MAX_INDEX_READS: usize = 2048;
const MAX_CURSOR: usize = 4096;

fn endpoint(e: ProxyEndpoint) -> &'static str {
    match e {
        ProxyEndpoint::Chat => "chat",
        ProxyEndpoint::Completions => "completions",
        ProxyEndpoint::Responses => "responses",
        ProxyEndpoint::Decisions => "decisions",
    }
}
fn rail(r: ProxyRail) -> &'static str {
    match r {
        ProxyRail::Fiat => "fiat",
        ProxyRail::Tap => "tap",
        ProxyRail::Tnk => "tnk",
    }
}
fn units(rates: &[ProxyRate]) -> String {
    // Length framing avoids ambiguous concatenation, independent of rate values.
    let mut hash = blake3::Hasher::new_derive_key("mayhem/proxy/candidate-unit-set/v1");
    for r in rates {
        hash.update(&(r.unit.len() as u64).to_le_bytes());
        hash.update(r.unit.as_bytes());
    }
    hash.finalize().to_hex().to_string()
}
/// Exact order for canonical rational AU/unit, including equivalent fractions.
/// Denominators <2^53 imply unequal fractions differ by >2^-106. Therefore 106
/// binary fractional digits preserve order; repeated doubling cannot overflow.
fn price_key(amount: u128, granularity: u64) -> Result<String> {
    require(
        granularity > 0 && granularity <= PROXY_MAX_SAFE_INTEGER,
        "invalid indexed granularity",
    )?;
    let denominator = u128::from(granularity);
    let mut remainder = amount % denominator;
    let mut fraction = 0u128;
    for _ in 0..106 {
        remainder *= 2;
        fraction = (fraction << 1) | (remainder / denominator);
        remainder %= denominator;
    }
    Ok(format!("{:032x}{fraction:027x}", amount / denominator))
}
fn offer_prefix(e: ProxyEndpoint, r: ProxyRail) -> String {
    format!("{ROOT}endpoint/{}/{}/", endpoint(e), rail(r))
}
fn keys(key: &str, value: &Value) -> Result<Vec<String>> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let suffix = key
        .strip_prefix(CATALOG_PREFIX)
        .ok_or_else(|| invalid("candidate row escaped catalog"))?;
    let mut out = Vec::new();
    if let Some(id) = suffix.strip_prefix("markets/") {
        let m: ProxyMarketDescriptor = serde_json::from_value(value.clone())?;
        out.push(format!(
            "{ROOT}family/{}/{id}",
            family_key(&m.model.family_id)
        ));
    } else if let Some(id) = suffix.strip_prefix("memberships/") {
        if value["active"] != true {
            return Ok(out);
        }
        let m: ProxyMembership = serde_json::from_value(value["member"].clone())?;
        out.push(format!("{ROOT}context/{:08x}/{id}", m.served_context));
    } else if let Some(id) = suffix.strip_prefix("offers/") {
        if value["active"] != true {
            return Ok(out);
        }
        let o: ProxyOffer = serde_json::from_value(value["offer"].clone())?;
        let ep = endpoint(o.endpoint);
        for r in &o.accepted_rails {
            out.push(format!("{}{id}", offer_prefix(o.endpoint, *r)));
        }
        out.push(format!("{ROOT}provider/{ep}/{}/{id}", o.provider_pubkey));
        out.push(format!("{ROOT}units/{ep}/{}/{id}", units(&o.rates)));
        out.push(format!("{ROOT}request/{ep}/{:032x}/{id}", o.per_request_au));
        out.push(format!("{ROOT}session/{ep}/{:032x}/{id}", o.min_session_au));
        for r in &o.rates {
            out.push(format!(
                "{ROOT}rate/{ep}/{}/{}/{id}",
                r.unit,
                price_key(r.per_unit_au, r.granularity)?
            ));
        }
    }
    Ok(out)
}
/// Row-local maintenance: membership/price changes never scan sibling offers.
/// Existing catalog transactions stage/commit this index with canonical rows.
pub(crate) fn update_index(
    index: &mut redb::Table<&str, &str>,
    key: &str,
    old: Option<&Value>,
    new: &Value,
) -> Result<()> {
    if let Some(old) = old {
        for k in keys(key, old)? {
            db(index.remove(k.as_str()))?;
        }
    }
    for k in keys(key, new)? {
        db(index.insert(k.as_str(), key))?;
    }
    Ok(())
}

#[derive(Clone)]
struct Span {
    lower: String,
    upper: String,
}
impl Span {
    fn prefix(prefix: String) -> Self {
        Self {
            upper: format!("{prefix}\u{7f}"),
            lower: prefix,
        }
    }
    fn through(prefix: String, maximum: &str) -> Self {
        Self {
            lower: prefix.clone(),
            upper: format!("{prefix}{maximum}/\u{7f}"),
        }
    }
    fn contains(&self, key: &str) -> bool {
        key >= self.lower.as_str() && key < self.upper.as_str()
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rows {
    Offers,
    Markets,
    Members,
}
struct Driver {
    name: &'static str,
    rows: Rows,
    spans: Vec<Span>,
}
fn drivers(policy: &Policy, selected_rail: ProxyRail) -> Result<Vec<Driver>> {
    let offers = offer_prefix(policy.endpoint, selected_rail);
    let mut list = vec![Driver {
        name: "endpoint_rail",
        rows: Rows::Offers,
        spans: vec![Span::prefix(offers.clone())],
    }];
    let (rows, spans) = match &policy.target {
        Target::ExactOffer { offer_id } => (
            Rows::Offers,
            vec![Span {
                lower: format!("{offers}{offer_id}"),
                upper: format!("{offers}{offer_id}\0"),
            }],
        ),
        Target::ExactMarket { market_id } => (
            Rows::Offers,
            vec![Span::prefix(format!("{offers}{market_id}/"))],
        ),
        Target::Category {
            family_ids,
            market_allowlist,
            ..
        } => match market_allowlist {
            Some(markets) => (
                Rows::Offers,
                markets
                    .iter()
                    .map(|id| Span::prefix(format!("{offers}{id}/")))
                    .collect(),
            ),
            None => (
                Rows::Markets,
                family_ids
                    .iter()
                    .map(|id| Span::prefix(format!("{ROOT}family/{}/", family_key(id))))
                    .collect(),
            ),
        },
    };
    list.push(Driver {
        name: "target",
        rows,
        spans,
    });
    let ep = endpoint(policy.endpoint);
    if let Some(providers) = &policy.providers.allow {
        list.push(Driver {
            name: "provider",
            rows: Rows::Offers,
            spans: providers
                .iter()
                .map(|p| Span::prefix(format!("{ROOT}provider/{ep}/{p}/")))
                .collect(),
        });
    }
    if let Some(context) = policy.constraints.minimum_context {
        list.push(Driver {
            name: "context",
            rows: Rows::Members,
            spans: vec![Span {
                lower: format!("{ROOT}context/{context:08x}/"),
                upper: format!("{ROOT}context/\u{7f}"),
            }],
        });
    }
    let p = &policy.prices;
    list.push(Driver {
        name: "unit_set",
        rows: Rows::Offers,
        spans: vec![Span::prefix(format!(
            "{ROOT}units/{ep}/{}/",
            units(&p.rates)
        ))],
    });
    for (name, value) in [("request", p.per_request_au), ("session", p.min_session_au)] {
        list.push(Driver {
            name,
            rows: Rows::Offers,
            spans: vec![Span::through(
                format!("{ROOT}{name}/{ep}/"),
                &format!("{value:032x}"),
            )],
        });
    }
    for rate in &p.rates {
        list.push(Driver {
            name: "unit_price",
            rows: Rows::Offers,
            spans: vec![Span::through(
                format!("{ROOT}rate/{ep}/{}/", rate.unit),
                &price_key(rate.per_unit_au, rate.granularity)?,
            )],
        });
    }
    Ok(list)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Child {
    parent: String,
    after: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    version: u32,
    query: String,
    snapshot: String,
    driver: usize,
    span: usize,
    after: Option<String>,
    child: Option<Child>,
}
impl Continuation {
    fn encode(&self) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }
}
fn query_key(policy: &Policy, selected_rail: ProxyRail) -> Result<String> {
    let body = stable_json_bytes(&serde_json::json!({"policy":policy,"rail":selected_rail}))?;
    let mut hash = blake3::Hasher::new_derive_key("mayhem/proxy/candidate-query/v1");
    hash.update(&(body.len() as u64).to_le_bytes());
    hash.update(&body);
    Ok(hash.finalize().to_hex().to_string())
}
fn seek(
    index: &redb::ReadOnlyTable<&str, &str>,
    span: &Span,
    after: Option<&str>,
    reads: &mut usize,
) -> Result<Option<(String, String)>> {
    *reads += 1;
    require(
        *reads <= MAX_INDEX_READS,
        "candidate index work exceeded bound",
    )?;
    let lower = after.map(Excluded).unwrap_or(Included(span.lower.as_str()));
    db(index.range::<&str>((lower, Excluded(span.upper.as_str()))))?
        .next()
        .map(|row| {
            let (k, v) = db(row)?;
            Ok((k.value().to_owned(), v.value().to_owned()))
        })
        .transpose()
}
fn choose(
    index: &redb::ReadOnlyTable<&str, &str>,
    drivers: &[Driver],
    policy: &Policy,
    selected_rail: ProxyRail,
    reads: &mut usize,
) -> Result<usize> {
    let mut best = (usize::MAX, usize::MAX, 0);
    for (id, driver) in drivers.iter().enumerate() {
        let mut count = 0;
        let mut parents = 0;
        for span in &driver.spans {
            let mut after = None;
            while count < PROBE_ROWS && parents < PROBE_ROWS {
                let Some((key, target)) = seek(index, span, after.as_deref(), reads)? else {
                    break;
                };
                after = Some(key);
                if driver.rows == Rows::Offers {
                    count += 1;
                } else {
                    parents += 1;
                    let nested = child_span(policy, selected_rail, driver.rows, &target)?;
                    let mut child_after = None;
                    while count < PROBE_ROWS {
                        let Some((key, _)) = seek(index, &nested, child_after.as_deref(), reads)?
                        else {
                            break;
                        };
                        child_after = Some(key);
                        count += 1;
                    }
                }
            }
            if count == PROBE_ROWS || parents == PROBE_ROWS {
                break;
            }
        }
        // Exhausting the parent sampling budget is unknown cardinality, not an
        // empty source. Never expand an unbounded market to plan a query.
        if parents == PROBE_ROWS {
            count = PROBE_ROWS;
        }
        let score = (count, usize::from(driver.rows != Rows::Offers), id);
        if score < best {
            best = score;
        }
        if count == 0 {
            break;
        }
    }
    Ok(best.2)
}
fn child_span(policy: &Policy, selected_rail: ProxyRail, rows: Rows, parent: &str) -> Result<Span> {
    let suffix = match rows {
        Rows::Markets => parent.strip_prefix(&format!("{CATALOG_PREFIX}markets/")),
        Rows::Members => parent.strip_prefix(&format!("{CATALOG_PREFIX}memberships/")),
        Rows::Offers => None,
    }
    .ok_or_else(|| invalid("invalid candidate parent"))?;
    let parts: Vec<_> = suffix.split('/').collect();
    require(
        parts.len() == (if rows == Rows::Markets { 1 } else { 2 }) && parts.iter().all(|v| hex(v)),
        "invalid candidate parent identity",
    )?;
    Ok(Span::prefix(format!(
        "{}{suffix}/",
        offer_prefix(policy.endpoint, selected_rail)
    )))
}
fn accepts(policy: &Policy, selected_rail: ProxyRail, p: &PublishedOffer) -> bool {
    let target = match &policy.target {
        Target::ExactOffer { offer_id } => p.id == *offer_id,
        Target::ExactMarket { market_id } => p.offer.market_id == *market_id,
        Target::Category {
            family_ids,
            market_allowlist,
            ..
        } => {
            family_ids.contains(&p.market.model.family_id)
                && market_allowlist
                    .as_ref()
                    .is_none_or(|v| v.contains(&p.offer.market_id))
        }
    };
    p.active
        && target
        && p.offer.endpoint == policy.endpoint
        && p.offer.accepted_rails.contains(&selected_rail)
        && (policy
            .providers
            .allow
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&p.offer.provider_pubkey))
            && !policy.providers.deny.contains(&p.offer.provider_pubkey))
        && policy
            .constraints
            .minimum_context
            .is_none_or(|n| p.membership.served_context >= n)
        && p.offer
            .validate_for_membership(&p.market, &p.membership)
            .is_ok()
        && policy.prices.permits(&p.offer).is_ok()
}

/// These are canonical candidates, not eligible routes. Each actual request
/// still requires descriptor/metering, availability, policy-evidence and budget
/// validation. Complete rate maps are retained for exact maximum quoting.
#[derive(Serialize)]
pub struct CandidatePage {
    pub schema_version: u32,
    pub query_key: String,
    pub snapshot: String,
    pub ordering: &'static str,
    pub index_driver: &'static str,
    pub entries: Vec<PublishedOffer>,
    pub next_cursor: Option<String>,
    /// Exhausts this traversal. A caller claiming a minimum must itself retain
    /// the complete progression from None; a supplied cursor is not that proof.
    pub exhausted: bool,
    pub scanned_candidates: usize,
    pub index_reads: usize,
    pub requires_observation_resolution: bool,
    pub requires_control_preparation: bool,
}
impl CatalogRead {
    /// Bounded incremental selection over one MVCC content snapshot. There is no
    /// price ranking or total catalog cap, and an empty page can have a cursor.
    pub fn proxy_candidates(
        &self,
        policy: &Policy,
        selected_rail: ProxyRail,
        cursor: Option<&str>,
        limit: usize,
        now_ms: u64,
    ) -> Result<CandidatePage> {
        policy.validate()?;
        require(
            policy.allowed_rails.contains(&selected_rail) && limit > 0 && limit <= MAX_PAGE_ENTRIES,
            "invalid candidate rail or page size",
        )?;
        let status = self.status();
        require(
            !status.invalidated && status.committed.is_some(),
            "proxy directory not hydrated",
        )?;
        let snapshot = status.content_snapshot;
        let query = query_key(policy, selected_rail)?;
        let drivers = drivers(policy, selected_rail)?;
        let index = db(self.tx.open_table(INDEX))?;
        let mut reads = 0;
        let mut c = if let Some(token) = cursor {
            let decode = || -> Result<Continuation> {
                require(token.len() <= MAX_CURSOR, "candidate cursor exceeds bound")?;
                let c: Continuation = serde_json::from_slice(
                    &URL_SAFE_NO_PAD
                        .decode(token)
                        .map_err(|_| invalid("invalid candidate cursor"))?,
                )?;
                require(
                    c.version == 1 && c.query == query && c.driver < drivers.len(),
                    "candidate cursor changed query",
                )?;
                if c.snapshot != snapshot {
                    return Ok(c);
                }
                let d = &drivers[c.driver];
                require(
                    c.span < d.spans.len()
                        && c.after
                            .as_ref()
                            .is_none_or(|k| k.len() <= 768 && d.spans[c.span].contains(k)),
                    "invalid candidate position",
                )?;
                if let Some(child) = &c.child {
                    let span = child_span(policy, selected_rail, d.rows, &child.parent)?;
                    require(
                        c.after.is_some()
                            && child
                                .after
                                .as_ref()
                                .is_none_or(|k| k.len() <= 768 && span.contains(k)),
                        "invalid candidate child position",
                    )?;
                    // The outer row must actually refer to this exact parent.
                    let parent = db(index.get(c.after.as_deref().unwrap()))?;
                    require(
                        parent.is_some_and(|p| p.value() == child.parent),
                        "candidate parent differs",
                    )?;
                }
                Ok(c)
            };
            let c = decode().map_err(|_| Error::DirectoryCursorInvalid)?;
            if c.snapshot != snapshot {
                return Err(Error::DirectoryCursorExpired);
            }
            c
        } else {
            Continuation {
                version: 1,
                query: query.clone(),
                snapshot: snapshot.clone(),
                driver: choose(&index, &drivers, policy, selected_rail, &mut reads)?,
                span: 0,
                after: None,
                child: None,
            }
        };
        let driver = &drivers[c.driver];
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut scanned = 0;
        loop {
            if c.span == driver.spans.len() {
                break;
            }
            if scanned == MAX_CANDIDATES || reads == MAX_INDEX_READS || entries.len() == limit {
                break;
            }
            let nested = c.child.is_some();
            let span = if let Some(child) = &c.child {
                child_span(policy, selected_rail, driver.rows, &child.parent)?
            } else {
                driver.spans[c.span].clone()
            };
            let after = c
                .child
                .as_ref()
                .map_or(c.after.as_deref(), |child| child.after.as_deref());
            let Some((key, target)) = seek(&index, &span, after, &mut reads)? else {
                if nested {
                    c.child = None;
                } else {
                    c.span += 1;
                    c.after = None;
                }
                continue;
            };
            scanned += 1;
            if !nested && driver.rows != Rows::Offers {
                child_span(policy, selected_rail, driver.rows, &target)?;
                c.after = Some(key);
                c.child = Some(Child {
                    parent: target,
                    after: None,
                });
                continue;
            }
            let id = target
                .strip_prefix(&format!("{CATALOG_PREFIX}offers/"))
                .ok_or_else(|| invalid("candidate index target differs"))?;
            let published = self
                .proxy_offer(id, now_ms)?
                .filter(|p| accepts(policy, selected_rail, p));
            if let Some(p) = published {
                let size = serde_json::to_vec(&p)?.len() + 1;
                require(
                    size <= MAX_PAGE_ENTRY_BYTES,
                    "candidate publication exceeds page bound",
                )?;
                if bytes + size > MAX_PAGE_ENTRY_BYTES {
                    break;
                }
                bytes += size;
                entries.push(p);
            }
            if let Some(child) = &mut c.child {
                child.after = Some(key);
            } else {
                c.after = Some(key);
            }
        }
        let exhausted = c.span == driver.spans.len();
        let next_cursor = if exhausted { None } else { Some(c.encode()?) };
        Ok(CandidatePage {
            schema_version: 1,
            query_key: query,
            snapshot,
            ordering: "index_traversal_not_cost",
            index_driver: driver.name,
            entries,
            next_cursor,
            exhausted,
            scanned_candidates: scanned,
            index_reads: reads,
            requires_observation_resolution: policy.requires_observation_resolution(),
            requires_control_preparation: !policy.constraints.request_controls.is_empty(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::price_key;
    #[test]
    fn rational_sort_keys_preserve_extremes_equivalent_values_and_adjacent_fractions() {
        let d = 9_007_199_254_740_991u64;
        assert_eq!(price_key(1, 3).unwrap(), price_key(2, 6).unwrap());
        assert!(price_key(u128::MAX, d).unwrap() < price_key(u128::MAX, d - 1).unwrap());
        assert!(price_key(u128::MAX - 1, d).unwrap() < price_key(u128::MAX, d).unwrap());
        // Consecutive fractions differ by only 1/(d*(d-1)), near 2^-106.
        assert!(
            price_key(u128::from(d - 2), d - 1).unwrap() < price_key(u128::from(d - 1), d).unwrap()
        );
        let mut state = 123456789u64;
        for _ in 0..10000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let a = state;
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let b = state;
            let x = (a % d).max(1);
            let y = (b % d).max(1);
            // u64 numerators times <2^53 denominators fit u128 exactly.
            assert_eq!(
                price_key(u128::from(a), x)
                    .unwrap()
                    .cmp(&price_key(u128::from(b), y).unwrap()),
                (u128::from(a) * u128::from(y)).cmp(&(u128::from(b) * u128::from(x)))
            );
        }
    }
}
