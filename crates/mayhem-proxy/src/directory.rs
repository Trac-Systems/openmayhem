//! Read-only public offer browsing. Every read uses one committed MVCC snapshot;
//! derived indexes and bounded keyset walks avoid whole-catalog/history scans.
//! Published rates are not execution quotes, and registration is not capacity.

pub mod candidates;

use std::ops::Bound::{Excluded, Included};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mayhem_proto::{
    proxy::{
        ProxyEndpoint, ProxyFamily, ProxyMarketDescriptor, ProxyMembership, ProxyOffer, ProxyRail,
    },
    stable_json_bytes,
};
use serde::{Deserialize, Serialize};

use crate::{
    attempts::Digest,
    catalog::{CatalogRead, CURRENT},
    db,
    discovery::{hex, CATALOG_PREFIX, MAX_PAGE_ENTRIES, MAX_PAGE_ENTRY_BYTES},
    invalid,
    matching::INDEX,
    presence::Registered,
    registry::publication::taxonomy::Model,
    require, Error, Result,
};

/// Work budget per page, not a limit on total reachable offers. Sparse filters
/// can return an empty page WITH a continuation; consumers must keep its cursor.
pub const MAX_CANDIDATES: usize = 256;
const MAX_CURSOR_BYTES: usize = 8192;

fn kind(value: ProxyFamily) -> &'static str {
    match value {
        ProxyFamily::Llm => "llm",
        ProxyFamily::Decisions => "decisions",
    }
}

fn text_key(value: &str) -> String {
    value
        .to_lowercase()
        .as_bytes()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}

fn family_key(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

/// Four bounded prefixes per immutable market, irrespective of provider count.
pub(crate) fn market_index_keys(market: &ProxyMarketDescriptor) -> Result<Vec<String>> {
    let id = market.id().map_err(invalid)?;
    let name = text_key(&market.model.model_id);
    let family = family_key(&market.model.family_id);
    let mut keys = Vec::with_capacity(4);
    for k in ["all", kind(market.family)] {
        for f in ["all", family.as_str()] {
            keys.push(format!("browse/{k}/{f}/{name}/{id}"));
        }
    }
    Ok(keys)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub kind: Option<ProxyFamily>,
    pub family_id: Option<String>,
    /// Case-insensitive model-name prefix. Not a substring or fuzzy search.
    #[serde(default)]
    pub name_prefix: String,
    pub endpoint: Option<ProxyEndpoint>,
    pub minimum_context: Option<u32>,
    pub rail: Option<ProxyRail>,
    /// Exact declared identity, never a capability or verified-weights claim.
    /// Omitted from ordinary query hashes for existing cursor compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
}

impl Query {
    fn validate(&self) -> Result<()> {
        if let Some(model) = &self.model {
            model
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
            require(
                self.family_id
                    .as_ref()
                    .is_none_or(|f| f == &model.family_id),
                "proxy model and family filters differ",
            )?;
        }
        require(
            self.name_prefix.len() <= 512 && !self.name_prefix.chars().any(char::is_control),
            "invalid proxy name prefix",
        )?;
        require(
            self.family_id.as_ref().is_none_or(|f| {
                !f.is_empty()
                    && f.len() <= 128
                    && f.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b)
                    })
            }),
            "invalid proxy family filter",
        )?;
        require(
            self.kind
                .is_none_or(|k| self.endpoint.is_none_or(|e| e.family() == k)),
            "proxy endpoint does not belong to this family",
        )
    }

    fn prefix(&self) -> String {
        let family = self
            .model
            .as_ref()
            .map(|m| &m.family_id)
            .or(self.family_id.as_ref())
            .map(String::as_str)
            .map(family_key)
            .unwrap_or("all".into());
        format!(
            "browse/{}/{}/{}",
            self.kind.map(kind).unwrap_or("all"),
            family,
            self.model
                .as_ref()
                .map(|m| format!("{}/", text_key(&m.model_id)))
                .unwrap_or_else(|| text_key(&self.name_prefix))
        )
    }

    pub fn key(&self) -> Result<String> {
        self.validate()?;
        let mut bytes = b"mayhem/proxy/directory-query/v1\0".to_vec();
        bytes.extend(stable_json_bytes(&serde_json::to_value(self)?)?);
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }

    fn accepts(&self, value: &PublishedOffer) -> bool {
        value.active
            && self.accepts_model(&value.market)
            && self.endpoint.is_none_or(|e| e == value.offer.endpoint)
            && self
                .minimum_context
                .is_none_or(|n| value.membership.served_context >= n)
            && self
                .rail
                .is_none_or(|r| value.offer.accepted_rails.contains(&r))
    }

    fn accepts_model(&self, market: &ProxyMarketDescriptor) -> bool {
        self.model.as_ref().is_none_or(|m| {
            m.family_id == market.model.family_id
                && m.model_id == market.model.model_id
                && m.revision == market.model.revision
                && m.quantization == market.model.quantization
                && m.model_id
                    .to_lowercase()
                    .starts_with(&self.name_prefix.to_lowercase())
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublishedOffer {
    /// Exactly market/provider/slot; labels and revisions are not identity.
    pub id: String,
    pub lane: &'static str,
    pub market: ProxyMarketDescriptor,
    pub membership: ProxyMembership,
    pub offer: ProxyOffer,
    pub digest: String,
    pub active: bool,
    /// Canonical policy/registration only. Live health, capacity, trust filters
    /// and the buyer's full constraints must still be evaluated at admission.
    pub catalog_eligible: bool,
    pub catalog_observed_at_ms: Option<u64>,
    /// No verified-operator claim is inferred from a provider's model name.
    pub operator_verification: &'static str,
    pub family_label: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct OfferPage {
    pub query_key: String,
    pub snapshot: String,
    pub entries: Vec<PublishedOffer>,
    pub previous_cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub scanned_candidates: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    market_key: String,
    /// None denotes an exhausted market; Some resumes inside its offer range.
    offer_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    query: String,
    snapshot: String,
    reverse: bool,
    #[serde(default)]
    inclusive: bool,
    position: Position,
}

fn encode(
    query: &str,
    snapshot: &str,
    reverse: bool,
    inclusive: bool,
    position: &Position,
) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
        version: 1,
        query: query.into(),
        snapshot: snapshot.into(),
        reverse,
        inclusive,
        position: position.clone(),
    })?))
}

fn decode(value: &str, query: &str, snapshot: &str, prefix: &str) -> Result<Cursor> {
    let parse = || -> Result<Cursor> {
        require(
            value.len() <= MAX_CURSOR_BYTES,
            "proxy cursor exceeds bound",
        )?;
        let c: Cursor = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| invalid("invalid proxy directory cursor"))?,
        )?;
        require(
            c.version == 1
                && c.query == query
                && c.position.market_key.starts_with(prefix)
                && c.position.market_key.len() <= 4096
                && c.position.market_key.bytes().all(|b| b.is_ascii_graphic()),
            "proxy cursor does not match this query",
        )?;
        let market_id = c.position.market_key.rsplit('/').next().unwrap_or("");
        require(hex(market_id), "invalid proxy cursor market")?;
        if let Some(key) = &c.position.offer_key {
            let offer_prefix = format!("{CATALOG_PREFIX}offers/");
            let id = key
                .strip_prefix(&offer_prefix)
                .ok_or_else(|| invalid("invalid proxy cursor offer"))?;
            let parts = offer_id(id)?;
            require(
                parts[0] == market_id,
                "proxy cursor offer belongs to another market",
            )?;
        }
        Ok(c)
    };
    let c = parse().map_err(|_| Error::DirectoryCursorInvalid)?;
    if c.snapshot != snapshot {
        return Err(Error::DirectoryCursorExpired);
    }
    Ok(c)
}

fn offer_id(value: &str) -> Result<[&str; 3]> {
    require(value.len() == 194, "invalid public proxy offer ID")?;
    let parts: Vec<_> = value.split('/').collect();
    require(
        parts.len() == 3 && parts.iter().all(|s| hex(s)),
        "invalid public proxy offer ID",
    )?;
    Ok([parts[0], parts[1], parts[2]])
}

impl CatalogRead {
    /// Exact lookup works independently of the current page, query or viewport.
    /// Inactive existing publications can be inspected; removed rows return None.
    pub fn proxy_offer(&self, id: &str, now_ms: u64) -> Result<Option<PublishedOffer>> {
        let [market, provider, slot] = offer_id(id)?;
        let Some(row) = self.get(&format!("{CATALOG_PREFIX}offers/{id}"))? else {
            return Ok(None);
        };
        let Some(market_row) = self.get(&format!("{CATALOG_PREFIX}markets/{market}"))? else {
            return Ok(None);
        };
        let Some(member_row) =
            self.get(&format!("{CATALOG_PREFIX}memberships/{market}/{provider}"))?
        else {
            return Ok(None);
        };
        let descriptor: ProxyMarketDescriptor = serde_json::from_value(market_row)?;
        let membership: ProxyMembership = serde_json::from_value(member_row["member"].clone())?;
        let offer: ProxyOffer = serde_json::from_value(row["offer"].clone())?;
        require(
            offer.market_id == market
                && offer.provider_pubkey == provider
                && offer.slot_id().map_err(invalid)? == slot
                && descriptor.id().map_err(invalid)? == market
                && membership.market_id == market
                && membership.provider_pubkey == provider,
            "stored proxy offer identity differs",
        )?;
        let digest = offer.digest().map_err(invalid)?;
        require(
            row["digest"].as_str() == Some(digest.as_str()),
            "stored proxy offer digest differs",
        )?;
        let family_label = self
            .get(&format!(
                "{CATALOG_PREFIX}families/{}",
                descriptor.model.family_id
            ))?
            .and_then(|v| v["label"].as_str().map(str::to_owned));
        let digest_id =
            |id: &str| Digest::new(id).map_err(|_| invalid("invalid proxy offer identity"));
        let eligible = Registered::read(
            self,
            &digest_id(market)?,
            &digest_id(provider)?,
            &digest_id(slot)?,
            now_ms,
        )
        .is_ok();
        Ok(Some(PublishedOffer {
            id: id.into(),
            lane: "proxy",
            market: descriptor,
            membership,
            offer,
            digest,
            active: row["active"] == true && member_row["active"] == true,
            catalog_eligible: eligible,
            catalog_observed_at_ms: self.status().committed.and_then(|s| s.observed_at_ms),
            operator_verification: "unknown",
            family_label,
        }))
    }

    /// Name-ordered indexed traversal, with bidirectional cursors. This does not
    /// subscribe to every provider or claim that a published offer has free slots.
    pub fn proxy_offers(
        &self,
        query: &Query,
        cursor: Option<&str>,
        limit: usize,
        now_ms: u64,
    ) -> Result<OfferPage> {
        self.proxy_offers_page(query, cursor, limit, now_ms, false)
    }

    /// Start at the final bounded page of a scope. Reverse continuation remains
    /// encoded in the normal query/snapshot-bound cursor, not a retained history.
    pub fn proxy_offers_from_end(
        &self,
        query: &Query,
        limit: usize,
        now_ms: u64,
    ) -> Result<OfferPage> {
        self.proxy_offers_page(query, None, limit, now_ms, true)
    }

    fn proxy_offers_page(
        &self,
        query: &Query,
        cursor: Option<&str>,
        limit: usize,
        now_ms: u64,
        from_end: bool,
    ) -> Result<OfferPage> {
        let query_key = query.key()?;
        require(
            limit > 0 && limit <= MAX_PAGE_ENTRIES,
            "invalid proxy directory page size",
        )?;
        let status = self.status();
        require(
            !status.invalidated && status.committed.is_some(),
            "proxy directory not hydrated",
        )?;
        let snapshot = status.content_snapshot;
        let prefix = query.prefix();
        let cursor = cursor
            .map(|c| decode(c, &query_key, &snapshot, &prefix))
            .transpose()?;
        let reverse = cursor.as_ref().map_or(from_end, |c| c.reverse);
        let inclusive = cursor.as_ref().is_some_and(|c| c.inclusive);
        let anchor = cursor.as_ref().map(|c| &c.position);
        let upper = format!("{prefix}\u{7f}");
        let market_bound = anchor.map(|p| {
            if p.offer_key.is_some() || inclusive {
                Included(p.market_key.as_str())
            } else {
                Excluded(p.market_key.as_str())
            }
        });
        let index = db(self.tx.open_table(INDEX))?;
        let range = if reverse {
            db(index.range::<&str>((
                Included(prefix.as_str()),
                market_bound.unwrap_or(Excluded(upper.as_str())),
            )))?
        } else {
            db(index.range::<&str>((
                market_bound.unwrap_or(Included(prefix.as_str())),
                Excluded(upper.as_str()),
            )))?
        };
        let markets: Box<dyn Iterator<Item = _>> = if reverse {
            Box::new(range.rev())
        } else {
            Box::new(range)
        };
        let offers = db(self.tx.open_table(CURRENT))?;
        let mut entries = Vec::new();
        let mut first = None;
        let mut last = anchor.cloned();
        let mut bytes = 0;
        let mut scanned = 0;
        let mut more = false;
        'markets: for item in markets {
            if scanned == MAX_CANDIDATES {
                more = true;
                break;
            }
            scanned += 1;
            let (market_key, market_value) = db(item)?;
            let market_key = market_key.value();
            let market = market_value.value().rsplit('/').next().unwrap_or("");
            require(hex(market), "invalid stored proxy directory market")?;
            if query.model.is_some() {
                let descriptor: ProxyMarketDescriptor = serde_json::from_value(
                    self.get(market_value.value())?
                        .ok_or_else(|| invalid("proxy directory market is missing"))?,
                )?;
                if !query.accepts_model(&descriptor) {
                    last = Some(Position {
                        market_key: market_key.into(),
                        offer_key: None,
                    });
                    continue;
                }
            }
            let offer_prefix = format!("{CATALOG_PREFIX}offers/{market}/");
            let offer_upper = format!("{offer_prefix}\u{7f}");
            let offer_anchor = anchor
                .filter(|p| p.market_key == market_key)
                .and_then(|p| p.offer_key.as_deref());
            let offer_bound = offer_anchor.map(|key| {
                if inclusive {
                    Included(key)
                } else {
                    Excluded(key)
                }
            });
            let range = if reverse {
                db(offers.range::<&str>((
                    Included(offer_prefix.as_str()),
                    offer_bound.unwrap_or(Excluded(offer_upper.as_str())),
                )))?
            } else {
                db(offers.range::<&str>((
                    offer_bound.unwrap_or(Included(offer_prefix.as_str())),
                    Excluded(offer_upper.as_str()),
                )))?
            };
            let rows: Box<dyn Iterator<Item = _>> = if reverse {
                Box::new(range.rev())
            } else {
                Box::new(range)
            };
            // A marker before this market's first row can resume without
            // skipping it if the work budget was consumed by empty markets.
            for item in rows {
                if scanned == MAX_CANDIDATES {
                    more = true;
                    break 'markets;
                }
                scanned += 1;
                let (key, _) = db(item)?;
                let key = key.value();
                let position = Position {
                    market_key: market_key.into(),
                    offer_key: Some(key.into()),
                };
                if let Some(value) = self.proxy_offer(
                    key.strip_prefix(&format!("{CATALOG_PREFIX}offers/"))
                        .unwrap_or(""),
                    now_ms,
                )? {
                    if query.accepts(&value) {
                        let size = serde_json::to_vec(&value)?.len() + 1;
                        require(
                            size <= MAX_PAGE_ENTRY_BYTES,
                            "proxy offer exceeds directory response bound",
                        )?;
                        if bytes + size > MAX_PAGE_ENTRY_BYTES {
                            more = true;
                            break 'markets;
                        }
                        bytes += size;
                        first.get_or_insert_with(|| position.clone());
                        entries.push(value);
                    }
                }
                last = Some(position);
                if entries.len() == limit {
                    more = true;
                    break 'markets;
                }
            }
            last = Some(Position {
                market_key: market_key.into(),
                offer_key: None,
            });
        }
        let continuing = if more {
            last.as_ref()
                .map(|p| encode(&query_key, &snapshot, reverse, false, p))
                .transpose()?
        } else {
            None
        };
        let returning = if anchor.is_some() {
            first
                .as_ref()
                .or(anchor)
                .map(|p| encode(&query_key, &snapshot, !reverse, first.is_none(), p))
                .transpose()?
        } else {
            None
        };
        let (previous_cursor, next_cursor) = if reverse {
            entries.reverse();
            (continuing, returning)
        } else {
            (returning, continuing)
        };
        Ok(OfferPage {
            query_key,
            snapshot,
            entries,
            previous_cursor,
            next_cursor,
            scanned_candidates: scanned,
        })
    }
}
