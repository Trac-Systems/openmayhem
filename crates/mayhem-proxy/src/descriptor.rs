//! Read-only public protocol descriptions over authenticated peer transport.
//! This channel cannot be promoted to negotiation/execution and carries no
//! request body, billing identity, capacity lease or signing operation.
use crate::{
    attempts::{Digest, Identity},
    discovery,
    endpoint::{Limits, PublicAdapter, PublicAdapterSnapshot},
    exchange::{
        self,
        channel::{bounded_json, Link, Purpose, Wire, WireMessage},
        Error, Role,
    },
    metering::Policy,
};
use mayhem_bridge::{sc_bridge_session_transport, ScBridgeConfig};
use mayhem_proto::{
    proxy::{ProxyEndpoint, ProxyOffer, ProxyRail},
    EndpointFamilyContract,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};

// The existing contract allowance is unchanged; the opt-in declaration has its
// own bound and a small JSON envelope allowance.
pub const MAX_BYTES: usize = 192 * 1024 + crate::declaration::MAX_BYTES + 64;
pub const READS: usize = 4;
pub const DEADLINE: Duration = Duration::from_secs(5);
pub(crate) const OPEN: &str = "p.describe.open";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub schema_version: u32,
    pub network: discovery::Identity,
    pub buyer: Digest,
    pub nonce: Digest,
    pub offer: ProxyOffer,
    pub rail: ProxyRail,
    pub settlement_policy_hash: Digest,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub data_handling: bool,
}
impl Context {
    pub fn new(
        local: &Identity,
        offer: ProxyOffer,
        rail: ProxyRail,
        policy: Digest,
    ) -> exchange::Result<Self> {
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce).map_err(|_| Error::Protocol)?;
        let context = Self {
            schema_version: 1,
            network: discovery::Identity {
                network_id: local.network_id.clone(),
                msb_bootstrap: local.msb_bootstrap.as_str().into(),
                subnet_bootstrap: local.subnet_bootstrap.as_str().into(),
                contract_version: mayhem_proto::CONTRACT_VERSION,
            },
            buyer: local.controller_pubkey.clone(),
            nonce: Digest::hash("mayhem/proxy/descriptor-nonce/v1", &[&nonce]),
            offer,
            rail,
            settlement_policy_hash: policy,
            data_handling: false,
        };
        context.link(local, Role::Buyer)?;
        Ok(context)
    }
    fn validate(&self) -> exchange::Result<()> {
        self.network.validate().map_err(|_| Error::Identity)?;
        self.offer.validate().map_err(|_| Error::Protocol)?;
        if self.schema_version != 1
            || self.network.contract_version != mayhem_proto::CONTRACT_VERSION
            || !self.offer.accepted_rails.contains(&self.rail)
        {
            return Err(Error::Protocol);
        }
        bounded_json(self, 32 * 1024)?;
        Ok(())
    }
    fn digest(&self) -> exchange::Result<Digest> {
        self.validate()?;
        let bytes = mayhem_proto::stable_json_bytes(
            &serde_json::to_value(self).map_err(|_| Error::Protocol)?,
        )
        .map_err(|_| Error::Protocol)?;
        Ok(Digest::hash(
            "mayhem/proxy/descriptor-context/v1",
            &[&bytes],
        ))
    }
    fn link(&self, local: &Identity, role: Role) -> exchange::Result<Link> {
        self.validate()?;
        local.validate().map_err(|_| Error::Identity)?;
        let (own, remote) = match role {
            Role::Buyer => (self.buyer.as_str(), self.offer.provider_pubkey.as_str()),
            Role::Provider => (self.offer.provider_pubkey.as_str(), self.buyer.as_str()),
        };
        if local.network_id != self.network.network_id
            || local.msb_bootstrap.as_str() != self.network.msb_bootstrap
            || local.subnet_bootstrap.as_str() != self.network.subnet_bootstrap
            || local.controller_pubkey.as_str() != own
        {
            return Err(Error::Identity);
        }
        Ok(Link {
            session_id: self.nonce.as_str().into(),
            remote: Digest::new(remote).map_err(|_| Error::Identity)?,
            role,
            purpose: Purpose::Descriptor(self.digest()?),
        })
    }
}

/// Only the already public contract and opaque recipe identity. Local resource
/// limits, upstream model mapping, addresses and credentials are excluded.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub schema_version: u32,
    pub endpoint: ProxyEndpoint,
    pub contract: EndpointFamilyContract,
    pub recipe_hash: Digest,
    pub metering_policy: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_handling: Option<crate::declaration::Signed>,
}
impl Descriptor {
    pub(crate) fn from_adapter(adapter: &crate::endpoint::Adapter) -> Self {
        let public = adapter.public_snapshot();
        Self {
            schema_version: 1,
            endpoint: public.endpoint,
            contract: public.contract,
            recipe_hash: public.recipe_hash,
            metering_policy: Policy::for_endpoint(public.endpoint).definition(),
            data_handling: None,
        }
    }
    pub fn adapter(
        &self,
        context: &Context,
        contract: &Digest,
        recipe: &Digest,
        limits: Limits,
    ) -> exchange::Result<PublicAdapterSnapshot> {
        if self.schema_version != 1
            || self.endpoint != context.offer.endpoint
            || &self.recipe_hash != recipe
            || self.metering_policy != Policy::for_endpoint(self.endpoint).definition()
            || context.offer.metering_policy_hash
                != Policy::for_endpoint(self.endpoint).hash().as_str()
        {
            return Err(Error::Identity);
        }
        let adapter = PublicAdapter::new(
            self.endpoint,
            self.contract.clone(),
            self.recipe_hash.clone(),
            limits,
        )
        .map_err(|_| Error::Protocol)?;
        if adapter.contract_hash() != contract {
            return Err(Error::Identity);
        }
        Ok(adapter.snapshot())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    t: String,
    context: Context,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Descriptor {
        context_digest: Digest,
        descriptor: Descriptor,
    },
    Unavailable {
        context_digest: Digest,
    },
}
impl WireMessage for Reply {
    fn sender(&self) -> Role {
        Role::Provider
    }
    fn validate(&self) -> exchange::Result<()> {
        bounded_json(self, MAX_BYTES).map(|_| ())
    }
}
/// Constructed only by the authenticated opening listener. No public constructor.
pub struct Incoming {
    context: Context,
    local: Identity,
    config: ScBridgeConfig,
    received: Instant,
}
impl Incoming {
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub(crate) fn from_event(
        event: &Value,
        local: &Identity,
        config: &ScBridgeConfig,
    ) -> exchange::Result<Self> {
        if event["type"] != "session_frame" || sc_bridge_session_transport(event).is_err() {
            return Err(Error::Identity);
        }
        let open: Open = serde_json::from_slice(&bounded_json(
            event.get("frame").ok_or(Error::Protocol)?,
            40 * 1024,
        )?)
        .map_err(|_| Error::Protocol)?;
        if open.t != OPEN
            || event["remote"] != open.context.buyer.as_str()
            || event["session_id"] != open.context.nonce.as_str()
        {
            return Err(Error::Identity);
        }
        open.context.link(local, Role::Provider)?;
        Ok(Self {
            context: open.context,
            local: local.clone(),
            config: config.clone(),
            received: Instant::now(),
        })
    }
    pub(crate) fn check(&self, local: &Identity) -> exchange::Result<()> {
        if local != &self.local || self.received.elapsed() >= deadline(&self.config)? {
            return Err(Error::Identity);
        }
        self.context.link(local, Role::Provider).map(|_| ())
    }
    pub(crate) async fn respond(self, descriptor: Option<Descriptor>) -> exchange::Result<()> {
        self.check(&self.local)?;
        let left = deadline(&self.config)?
            .checked_sub(self.received.elapsed())
            .ok_or(Error::Interrupted)?;
        tokio::time::timeout(left, async {
            let mut wire = Wire::connect(
                self.config.clone(),
                self.context.link(&self.local, Role::Provider)?,
                exchange::Limits {
                    max_message_bytes: MAX_BYTES,
                },
            )
            .await?;
            let context_digest = self.context.digest()?;
            let reply = match descriptor {
                Some(descriptor) => Reply::Descriptor {
                    context_digest,
                    descriptor,
                },
                None => Reply::Unavailable { context_digest },
            };
            wire.send(&reply).await?;
            wire.close().await
        })
        .await
        .map_err(|_| Error::Interrupted)?
    }
}
fn deadline(config: &ScBridgeConfig) -> exchange::Result<Duration> {
    config
        .operation_deadline
        .filter(|d| !d.is_zero())
        .map(|d| d.min(DEADLINE))
        .ok_or(Error::Protocol)
}
pub async fn fetch(
    config: ScBridgeConfig,
    local: &Identity,
    context: &Context,
) -> exchange::Result<Descriptor> {
    let wait = deadline(&config)?;
    tokio::time::timeout(wait, async {
        let mut wire = Wire::connect(
            config,
            context.link(local, Role::Buyer)?,
            exchange::Limits {
                max_message_bytes: MAX_BYTES,
            },
        )
        .await?;
        wire.opening_send(
            serde_json::to_value(Open {
                t: OPEN.into(),
                context: context.clone(),
            })
            .map_err(|_| Error::Protocol)?,
        )
        .await?;
        let reply: Reply = wire.receive(Some(wait)).await?;
        wire.close().await?;
        match reply {
            Reply::Descriptor {
                context_digest,
                descriptor,
            } if context_digest == context.digest()? => Ok(descriptor),
            _ => Err(Error::Protocol),
        }
    })
    .await
    .map_err(|_| Error::Interrupted)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (Identity, Identity, Context, Descriptor) {
        let d = |n: u8| Digest::hash("descriptor-test", &[&[n]]);
        let buyer = Identity {
            network_id: "descriptor-test".into(),
            msb_bootstrap: d(1),
            subnet_bootstrap: d(2),
            controller_pubkey: d(3),
        };
        let provider = Identity {
            controller_pubkey: d(4),
            ..buyer.clone()
        };
        let contract = mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
        )
        .unwrap();
        let adapter = crate::endpoint::Adapter::new(
            ProxyEndpoint::Chat,
            contract,
            "private-upstream-model".into(),
            limits(),
        )
        .unwrap();
        let offer = ProxyOffer {
            schema_version: 1,
            lane: mayhem_proto::proxy::ProxyLane::Proxy,
            market_id: d(5).as_str().into(),
            provider_pubkey: d(4).as_str().into(),
            membership_revision: 1,
            revision: 1,
            endpoint: ProxyEndpoint::Chat,
            ctx_bracket: "le128k".into(),
            outcome_class: String::new(),
            metering_policy_hash: Policy::ObservableTextV1.hash().as_str().into(),
            rates: vec![
                mayhem_proto::proxy::ProxyRate {
                    unit: "input_token".into(),
                    per_unit_au: 1,
                    granularity: 1,
                },
                mayhem_proto::proxy::ProxyRate {
                    unit: "output_token".into(),
                    per_unit_au: 1,
                    granularity: 1,
                },
            ],
            per_request_au: 0,
            min_session_au: 0,
            accepted_rails: vec![ProxyRail::Fiat],
        };
        let context = Context::new(&buyer, offer, ProxyRail::Fiat, d(6)).unwrap();
        (buyer, provider, context, Descriptor::from_adapter(&adapter))
    }
    fn limits() -> Limits {
        Limits {
            request_bytes: 65536,
            response_bytes: 65536,
            choices: 8,
            tools: 8,
            questions: 8,
            decision_options: 8,
        }
    }
    #[test]
    fn descriptor_open_requires_authenticated_peer_network_nonce_and_current_contract() {
        let (_, provider, context, _) = fixture();
        let config = ScBridgeConfig::new("ws://127.0.0.1:1", "fixture-only")
            .unwrap()
            .with_operation_deadline(Some(Duration::from_secs(1)));
        let event = json!({"type":"session_frame","remote":context.buyer,"session_id":context.nonce,
            "direct":true,"relayed":false,"frame":{"t":OPEN,"context":context}});
        assert!(Incoming::from_event(&event, &provider, &config).is_ok());
        for path in ["remote", "session_id", "direct"] {
            let mut wrong = event.clone();
            wrong[path] = if path == "direct" {
                json!(false)
            } else {
                json!("f".repeat(64))
            };
            assert!(
                Incoming::from_event(&wrong, &provider, &config).is_err(),
                "{path}"
            );
        }
        for (path, value) in [
            ("network_id", json!("another-network")),
            ("contract_version", json!(0)),
        ] {
            let mut wrong = event.clone();
            wrong["frame"]["context"]["network"][path] = value;
            assert!(Incoming::from_event(&wrong, &provider, &config).is_err());
        }
    }
    #[test]
    fn public_descriptor_hash_checks_omit_private_config_and_ignore_provider_limits() {
        let (_, _, context, descriptor) = fixture();
        let contract = Digest::new(mayhem_proto::endpoint_contract_canonical_fingerprint(
            &descriptor.contract,
        ))
        .unwrap();
        let encoded = serde_json::to_string(&descriptor).unwrap();
        assert!(!encoded.contains("private-upstream-model"));
        assert!(!encoded.contains("limits"));
        assert!(descriptor
            .adapter(&context, &contract, &descriptor.recipe_hash, limits())
            .is_ok());
        let wrong = Digest::hash("descriptor-wrong", &[]);
        assert!(descriptor
            .adapter(&context, &wrong, &descriptor.recipe_hash, limits())
            .is_err());
        assert!(descriptor
            .adapter(&context, &contract, &wrong, limits())
            .is_err());
        let mut changed = descriptor.clone();
        changed.contract.request_attributes.push("another".into());
        assert!(changed
            .adapter(&context, &contract, &descriptor.recipe_hash, limits())
            .is_err());
        changed = descriptor.clone();
        changed.metering_policy["algorithm"] = json!("provider-invented");
        assert!(changed
            .adapter(&context, &contract, &descriptor.recipe_hash, limits())
            .is_err());
    }
}
