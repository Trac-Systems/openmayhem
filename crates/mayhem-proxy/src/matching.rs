//! Deterministic setup suggestions, not automatic membership or model identity
//! attestation. Queries use a derived on-disk index from one complete catalog.

use crate::{
    catalog::CatalogRead,
    db,
    discovery::{hex, Proof, CATALOG_PREFIX, MAX_PAGE_ENTRY_BYTES},
    invalid, require, Result,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use mayhem_proto::{
    proxy::{
        ProxyEndpointContract, ProxyFamily, ProxyLane, ProxyMarketDescriptor,
        ProxyMeteringContract, ProxyModelClaim, ProxyPricing,
    },
    stable_json_bytes,
};
use redb::TableDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) const INDEX: TableDefinition<&str, &str> =
    TableDefinition::new("proxy_match_current_v1");
pub(crate) const STAGED_INDEX: TableDefinition<&str, &str> =
    TableDefinition::new("proxy_match_staged_v1");
pub(crate) const INDEX_VERSION: u32 = 1;
const MAX_SCAN: usize = 200;

fn digest(value: &impl Serialize) -> Result<String> {
    let mut bytes = b"mayhem/proxy/match/v1\0".to_vec();
    bytes.extend(stable_json_bytes(&serde_json::to_value(value)?)?);
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn normalized_name(name: &str) -> String {
    // Deliberately conservative and versioned. Unicode is preserved; ASCII
    // separators/case affect a NAME suggestion only, never an exact identity.
    name.chars()
        .filter(|c| !c.is_ascii_whitespace() && !matches!(c, '-' | '_' | '/' | '.'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn compatibility(
    family: ProxyFamily,
    endpoints: &[ProxyEndpointContract],
    metering: &ProxyMeteringContract,
) -> Result<String> {
    digest(&json!({"family":family,"endpoints":endpoints,"metering":metering}))
}

fn name_digest(model: &ProxyModelClaim) -> Result<String> {
    digest(&json!({"family_id":model.family_id,"name":normalized_name(&model.model_id)}))
}

pub(crate) fn index_keys(market: &ProxyMarketDescriptor) -> Result<Vec<String>> {
    market.validate().map_err(invalid)?;
    let id = market.id().map_err(invalid)?;
    let mut keys = Vec::new();
    // There are at most three declared endpoints. A provider may join with a
    // supported subset; pre-index the at most seven nonempty subsets, rather
    // than scanning every market for each setup request.
    for mask in 1..(1usize << market.endpoints.len()) {
        let endpoints: Vec<_> = market
            .endpoints
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, e)| e.clone())
            .collect();
        let compatible = compatibility(market.family, &endpoints, &market.metering)?;
        keys.push(format!(
            "exact/{compatible}/{}/{id}",
            digest(&market.model)?
        ));
        keys.push(format!(
            "name/{compatible}/{}/{id}",
            name_digest(&market.model)?
        ));
    }
    Ok(keys)
}

pub(crate) fn update_index(
    index: &mut redb::Table<&str, &str>,
    key: &str,
    old: Option<&Value>,
    new: &Value,
) -> Result<()> {
    if !key.starts_with(&format!("{CATALOG_PREFIX}markets/")) {
        return Ok(());
    }
    if let Some(old) = old.filter(|v| !v.is_null()) {
        let market: ProxyMarketDescriptor = serde_json::from_value(old.clone())?;
        for index_key in index_keys(&market)? {
            db(index.remove(index_key.as_str()))?;
        }
    }
    if !new.is_null() {
        let market: ProxyMarketDescriptor = serde_json::from_value(new.clone())?;
        for index_key in index_keys(&market)? {
            db(index.insert(index_key.as_str(), key))?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionRequest {
    pub schema_version: u32,
    /// Digest of the saved discovery/conformance report supplying these claims.
    /// This reference alone does not authenticate or prove the report's contents.
    pub report_digest: String,
    pub family: ProxyFamily,
    pub model: ProxyModelClaim,
    pub endpoints: Vec<ProxyEndpointContract>,
    pub metering: ProxyMeteringContract,
    /// Explicit declared aliases. Never guessed by inference in the request path.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl SuggestionRequest {
    pub fn validate(&self) -> Result<()> {
        require(
            self.schema_version == 1
                && hex(&self.report_digest)
                && serde_json::to_vec(self)?.len() <= 16 * 1024,
            "invalid saved suggestion request",
        )?;
        require(
            !normalized_name(&self.model.model_id).is_empty(),
            "model identity is undisclosed; browse compatible markets and choose explicitly",
        )?;
        require(
            self.aliases.len() <= 16 && self.aliases.windows(2).all(|v| v[0] < v[1]),
            "model aliases must be sorted, distinct and bounded",
        )?;
        let validate = |model| {
            ProxyMarketDescriptor {
                schema_version: 1,
                lane: ProxyLane::Proxy,
                creator_pubkey: "0".repeat(64),
                slug: "suggestion-validation".into(),
                model,
                family: self.family,
                endpoints: self.endpoints.clone(),
                metering: self.metering.clone(),
                pricing: ProxyPricing::ProviderOffers,
            }
            .validate()
            .map_err(invalid)
        };
        validate(self.model.clone())?;
        for alias in &self.aliases {
            require(!normalized_name(alias).is_empty(), "model alias is empty")?;
            let mut model = self.model.clone();
            model.model_id = alias.clone();
            validate(model)?;
        }
        Ok(())
    }

    fn prefixes(&self, rank: u8) -> Result<Vec<String>> {
        let compatible = compatibility(self.family, &self.endpoints, &self.metering)?;
        let mut models = vec![self.model.clone()];
        for alias in &self.aliases {
            let mut model = self.model.clone();
            model.model_id = alias.clone();
            models.push(model);
        }
        let mut result = BTreeSet::new();
        match rank {
            0 => {
                result.insert(format!("exact/{compatible}/{}/", digest(&self.model)?));
            }
            1 => {
                for model in models.iter().skip(1) {
                    result.insert(format!("exact/{compatible}/{}/", digest(model)?));
                }
            }
            2 => {
                for model in &models {
                    result.insert(format!("name/{compatible}/{}/", name_digest(model)?));
                }
            }
            _ => return Err(invalid("invalid suggestion cursor rank")),
        }
        Ok(result.into_iter().collect())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    ExactDeclaredIdentity,
    DeclaredAlias,
    NormalizedName,
    ConflictingClaim,
}

#[derive(Clone, Debug, Serialize)]
pub struct Suggestion {
    pub market_id: String,
    pub market: ProxyMarketDescriptor,
    pub match_kind: MatchKind,
    pub reasons: Vec<&'static str>,
    pub conflicts: Vec<&'static str>,
    pub requires_explicit_selection: bool,
}

fn classify(
    request: &SuggestionRequest,
    market: ProxyMarketDescriptor,
) -> Result<(u8, Suggestion)> {
    require(
        market.family == request.family
            && market.metering == request.metering
            && request
                .endpoints
                .iter()
                .all(|e| market.endpoints.contains(e))
            && market.model.family_id == request.model.family_id,
        "suggestion index references an incompatible market",
    )?;
    let mut conflicts = Vec::new();
    if !request.model.revision.is_empty() && market.model.revision != request.model.revision {
        conflicts.push("revision_mismatch_or_undisclosed");
    }
    if !request.model.quantization.is_empty()
        && market.model.quantization != request.model.quantization
    {
        conflicts.push("quantization_mismatch_or_undisclosed");
    }
    let exact = market.model == request.model;
    let alias = request.aliases.contains(&market.model.model_id)
        && market.model.revision == request.model.revision
        && market.model.quantization == request.model.quantization;
    let (rank, kind) = if exact {
        (0, MatchKind::ExactDeclaredIdentity)
    } else if alias {
        (1, MatchKind::DeclaredAlias)
    } else if !conflicts.is_empty() {
        (2, MatchKind::ConflictingClaim)
    } else {
        (2, MatchKind::NormalizedName)
    };
    let mut reasons = vec![
        "endpoint_contracts_compatible",
        "metering_contract_matches",
        "declared_identity_is_not_verified_weights",
    ];
    if market.model.revision.is_empty() {
        reasons.push("revision_undisclosed");
    }
    if market.model.quantization.is_empty() {
        reasons.push("quantization_undisclosed");
    }
    Ok((
        rank,
        Suggestion {
            market_id: market.id().map_err(invalid)?,
            market,
            match_kind: kind,
            reasons,
            conflicts,
            requires_explicit_selection: true,
        },
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    query_hash: String,
    proof: Proof,
    rank: u8,
    after: Option<String>,
}

impl Cursor {
    fn encode(&self) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }
    fn decode(value: &str, query_hash: &str, proof: &Proof) -> Result<Self> {
        require(value.len() <= 2048, "suggestion cursor exceeds bound")?;
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| invalid("invalid suggestion cursor"))?;
        let cursor: Self = serde_json::from_slice(&bytes)?;
        require(
            cursor.version == 1
                && cursor.query_hash == query_hash
                && cursor.proof == *proof
                && cursor.rank <= 2
                && cursor.after.as_deref().is_none_or(hex),
            "suggestion cursor changed query or snapshot; restart paging",
        )?;
        Ok(cursor)
    }
}

#[derive(Serialize)]
pub struct SuggestionPage {
    pub schema_version: u32,
    pub query_hash: String,
    pub proof: Proof,
    pub entries: Vec<Suggestion>,
    pub next_cursor: Option<String>,
}

fn next(
    index: &redb::ReadOnlyTable<&str, &str>,
    prefix: &str,
    after: Option<&str>,
) -> Result<Option<String>> {
    use std::ops::Bound::{Excluded, Included};
    let start = format!("{prefix}{}", after.unwrap_or(""));
    let end = format!("{prefix}\u{7f}");
    let lower = if after.is_some() {
        Excluded(start.as_str())
    } else {
        Included(start.as_str())
    };
    db(index.range::<&str>((lower, Excluded(end.as_str()))))?
        .next()
        .map(|entry| {
            let (key, _) = db(entry)?;
            let id = key
                .value()
                .rsplit('/')
                .next()
                .ok_or_else(|| invalid("invalid suggestion index key"))?;
            require(hex(id), "invalid suggestion market ID")?;
            Ok(id.to_owned())
        })
        .transpose()
}

impl CatalogRead {
    /// Bounded indexed setup query: exact declarations, explicit aliases, then
    /// normalized names/conflicts. Stable market IDs break ties within each rank.
    /// Pagination has no total result cap; sparse pages can carry a continuation.
    pub fn suggest(
        &self,
        request: &SuggestionRequest,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<SuggestionPage> {
        request.validate()?;
        require(limit > 0 && limit <= 100, "invalid suggestion page size")?;
        let status = self.status();
        require(
            !status.invalidated,
            "catalog requires a complete refresh before suggestions",
        )?;
        let proof = status
            .committed
            .ok_or_else(|| invalid("catalog has not been hydrated"))?
            .proof;
        let query_hash = digest(request)?;
        let mut cursor = match cursor {
            Some(value) => Cursor::decode(value, &query_hash, &proof)?,
            None => Cursor {
                version: 1,
                query_hash: query_hash.clone(),
                proof: proof.clone(),
                rank: 0,
                after: None,
            },
        };
        let index = db(self.tx.open_table(INDEX))?;
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut scanned = 0;
        loop {
            let prefixes = request.prefixes(cursor.rank)?;
            let mut heads = prefixes
                .iter()
                .map(|p| next(&index, p, cursor.after.as_deref()))
                .collect::<Result<Vec<_>>>()?;
            while let Some(id) = heads.iter().filter_map(|v| v.as_ref()).min().cloned() {
                let market: ProxyMarketDescriptor = serde_json::from_value(
                    self.get(&format!("{CATALOG_PREFIX}markets/{id}"))?
                        .ok_or_else(|| invalid("suggestion index references a missing market"))?,
                )?;
                let (rank, suggestion) = classify(request, market)?;
                let size = serde_json::to_vec(&suggestion)?.len();
                if rank == cursor.rank && bytes + size > MAX_PAGE_ENTRY_BYTES {
                    return Ok(SuggestionPage {
                        schema_version: 1,
                        query_hash,
                        proof,
                        entries,
                        next_cursor: Some(cursor.encode()?),
                    });
                }
                for (i, head) in heads.iter_mut().enumerate() {
                    if head.as_ref() == Some(&id) {
                        *head = next(&index, &prefixes[i], Some(&id))?;
                    }
                }
                scanned += 1;
                cursor.after = Some(id);
                if rank == cursor.rank {
                    bytes += size;
                    entries.push(suggestion);
                }
                if entries.len() == limit || scanned == MAX_SCAN {
                    let more = cursor.rank < 2 || heads.iter().any(Option::is_some);
                    return Ok(SuggestionPage {
                        schema_version: 1,
                        query_hash,
                        proof,
                        entries,
                        next_cursor: if more { Some(cursor.encode()?) } else { None },
                    });
                }
            }
            if cursor.rank == 2 {
                break;
            }
            cursor.rank += 1;
            cursor.after = None;
        }
        Ok(SuggestionPage {
            schema_version: 1,
            query_hash,
            proof,
            entries,
            next_cursor: None,
        })
    }
}
