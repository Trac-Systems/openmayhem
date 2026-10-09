//! Public provider promises. Signatures establish attribution, never that a
//! remote service obeys a privacy promise. These values have Declared assurance.
use crate::{attempts::Digest, discovery, registry, require, Result};
use mayhem_proto::proxy::{ProxyEndpoint, ProxyMembership, ProxyOffer, PROXY_MAX_SAFE_INTEGER};
use serde::{Deserialize, Serialize};

pub const MAX_BYTES: usize = 16 * 1024;
pub const MAX_RECORDS: usize = 32;
pub const MAX_READ_AGE_MS: u64 = 15_000;
const DOMAIN: &str = "mayhem/proxy/provider-data-handling/v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub network: discovery::Identity,
    pub provider: Digest,
    pub market: Digest,
    pub membership_revision: u64,
    pub membership_digest: Digest,
    pub endpoint: ProxyEndpoint,
    pub endpoint_contract: Digest,
    pub recipe_hash: Digest,
    pub connection_revision: u64,
}
impl Subject {
    pub fn new(
        network: discovery::Identity,
        offer: &ProxyOffer,
        member: &ProxyMembership,
    ) -> Result<Self> {
        network.validate()?;
        require(
            offer.provider_pubkey == member.provider_pubkey
                && offer.market_id == member.market_id
                && offer.membership_revision == member.revision,
            "declaration membership differs",
        )?;
        let contract = member
            .endpoints
            .iter()
            .find(|e| e.endpoint == offer.endpoint)
            .ok_or_else(|| crate::invalid("declaration endpoint missing"))?;
        let digest = |s| Digest::new(s).map_err(|_| crate::invalid("invalid declaration identity"));
        Ok(Self {
            network,
            provider: digest(member.provider_pubkey.clone())?,
            market: digest(member.market_id.clone())?,
            membership_revision: member.revision,
            membership_digest: digest(member.digest().map_err(crate::invalid)?)?,
            endpoint: offer.endpoint,
            endpoint_contract: digest(contract.contract_hash.clone())?,
            recipe_hash: digest(member.recipe_hash.clone())?,
            connection_revision: member.connection_revision,
        })
    }
    fn validate(&self) -> Result<()> {
        self.network.validate()?;
        require(
            self.membership_revision > 0
                && self.membership_revision <= PROXY_MAX_SAFE_INTEGER
                && self.connection_revision > 0
                && self.connection_revision <= PROXY_MAX_SAFE_INTEGER,
            "invalid declaration revision",
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub field_id: String,
    pub schema_revision: u32,
    pub definition_digest: Digest,
    pub status: registry::Support,
    pub value: Option<registry::TypedValue>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    pub schema_version: u32,
    pub subject: Subject,
    pub revision: u64,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub claims: Vec<Claim>,
}
impl Body {
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        self.subject.validate()?;
        require(
            self.schema_version == 1
                && self.revision > 0
                && self.revision <= PROXY_MAX_SAFE_INTEGER
                && self.issued_at_ms < self.expires_at_ms
                && self.expires_at_ms <= PROXY_MAX_SAFE_INTEGER
                && !self.claims.is_empty()
                && self.claims.len() <= 32,
            "invalid declaration bounds",
        )?;
        let mut previous = None;
        for c in &self.claims {
            require(
                registry::identifier(&c.field_id)
                    && c.schema_revision > 0
                    && c.schema_revision <= i32::MAX as u32
                    && previous.is_none_or(|p: &str| p < c.field_id.as_str())
                    && (c.status == registry::Support::Supported) == c.value.is_some(),
                "invalid declaration claim",
            )?;
            previous = Some(c.field_id.as_str());
            if let Some(v) = &c.value {
                v.validate()?;
            }
        }
        let bytes = mayhem_proto::stable_json_bytes(&serde_json::to_value(self)?)?;
        require(
            bytes.len() <= MAX_BYTES - 512,
            "declaration exceeds byte bound",
        )?;
        let mut signed = DOMAIN.as_bytes().to_vec();
        signed.push(0);
        signed.extend(bytes);
        Ok(signed)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signed {
    pub body: Body,
    pub signature: String,
}
impl Signed {
    pub fn verify(&self) -> Result<()> {
        require(
            serde_json::to_vec(self)?.len() <= MAX_BYTES
                && crate::receipts::verify_signature(
                    &self.signature,
                    &self.body.signing_bytes()?,
                    self.body.subject.provider.as_str(),
                ),
            "provider declaration signature rejected",
        )
    }
    pub fn digest(&self) -> Result<Digest> {
        self.verify()?;
        Ok(Digest::hash(
            "mayhem/proxy/provider-data-handling-record/v1",
            &[&mayhem_proto::stable_json_bytes(&serde_json::to_value(
                self,
            )?)?],
        ))
    }
    pub fn check(&self, subject: &Subject, now: u64) -> Result<()> {
        self.verify()?;
        require(
            &self.body.subject == subject
                && self.body.issued_at_ms <= now
                && now < self.body.expires_at_ms,
            "provider declaration is stale or belongs to another route",
        )
    }
    pub fn evaluate(
        &self,
        definition: &registry::Definition,
        predicate: &registry::Predicate,
        now: u64,
    ) -> Result<registry::Match> {
        self.verify()?;
        require(
            matches!(definition.usage, registry::Usage::FilterOnly),
            "data-handling field must be filter-only",
        )?;
        let claim = self.body.claims.iter().find(|c| {
            c.field_id == predicate.field_id && c.schema_revision == predicate.schema_revision
        });
        if let Some(c) = claim {
            require(
                c.definition_digest.as_str() == definition.digest()?,
                "declaration definition differs",
            )?;
        }
        let observation = claim.map(|c| registry::Observation {
            field_id: c.field_id.clone(),
            schema_revision: c.schema_revision,
            endpoint: self.body.subject.endpoint,
            status: c.status,
            value: c.value.clone(),
            observed_at_ms: self.body.issued_at_ms,
            expires_at_ms: Some(self.body.expires_at_ms),
            source_id: "provider_declaration".into(),
        });
        registry::evaluate(
            definition,
            predicate,
            observation.as_ref(),
            registry::Assurance::Declared,
            self.body.subject.endpoint,
            now,
        )
    }
    /// Bounded rules inspect the same signed claim set and pinned definitions.
    /// Missing values never make a conditional requirement disappear.
    pub fn evaluate_with_rules<'a>(
        &self,
        definition: &registry::Definition,
        predicate: &registry::Predicate,
        lookup: impl Fn(&str, u32) -> Option<&'a registry::Definition>,
        now: u64,
    ) -> Result<registry::Match> {
        let matched = self.evaluate(definition, predicate, now)?;
        if matched != registry::Match::Satisfied {
            return Ok(matched);
        }
        for rule in &definition.rules {
            let assess = |c: &registry::Condition| -> Result<registry::Match> {
                let d = lookup(&c.field_id, c.schema_revision)
                    .ok_or_else(|| crate::invalid("declaration rule definition missing"))?;
                self.evaluate(
                    d,
                    &registry::Predicate {
                        field_id: c.field_id.clone(),
                        schema_revision: c.schema_revision,
                        operator: c.operator,
                        value: c.value.clone(),
                        evidence: predicate.evidence,
                        max_age_ms: predicate.max_age_ms,
                    },
                    now,
                )
            };
            let when = rule.when.iter().map(&assess).collect::<Result<Vec<_>>>()?;
            if when.contains(&registry::Match::DifferentValue) {
                continue;
            }
            if when.iter().any(|m| *m != registry::Match::Satisfied) {
                return Ok(registry::Match::Unknown);
            }
            for c in &rule.require {
                let result = assess(c)?;
                if result != registry::Match::Satisfied {
                    return Ok(result);
                }
            }
            for c in &rule.forbid {
                match assess(c)? {
                    registry::Match::DifferentValue => {}
                    registry::Match::Satisfied => return Ok(registry::Match::DifferentValue),
                    _ => return Ok(registry::Match::Unknown),
                }
            }
        }
        Ok(registry::Match::Satisfied)
    }
}
