//! Buyer-owned public protocol evidence. The recipe digest is an opaque identity
//! checked against canonical membership, not a provider recipe to execute locally.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicAdapterSnapshot {
    pub version: u32,
    pub endpoint: ProxyEndpoint,
    pub contract: EndpointFamilyContract,
    pub recipe_hash: Digest,
    /// Buyer's local resource bounds; not part of the provider's recipe digest.
    pub limits: Limits,
}
impl fmt::Debug for PublicAdapterSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicAdapterSnapshot")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

pub struct PublicAdapter {
    protocol: Protocol,
}
impl PublicAdapter {
    pub fn restore(snapshot: PublicAdapterSnapshot) -> Result<Self> {
        if snapshot.version != 1 {
            return Err(Error::Configuration);
        }
        Self::new(
            snapshot.endpoint,
            snapshot.contract,
            snapshot.recipe_hash,
            snapshot.limits,
        )
    }
    pub fn new(
        endpoint: ProxyEndpoint,
        contract: EndpointFamilyContract,
        recipe_hash: Digest,
        limits: Limits,
    ) -> Result<Self> {
        let contract_hash = validate_contract(endpoint, &contract, limits)?;
        Ok(Self {
            protocol: Protocol {
                endpoint,
                contract,
                contract_hash,
                recipe_hash,
                limits,
            },
        })
    }
    pub fn snapshot(&self) -> PublicAdapterSnapshot {
        PublicAdapterSnapshot {
            version: 1,
            endpoint: self.protocol.endpoint,
            contract: self.protocol.contract.clone(),
            recipe_hash: self.protocol.recipe_hash.clone(),
            limits: self.protocol.limits,
        }
    }
    pub fn endpoint(&self) -> ProxyEndpoint {
        self.protocol.endpoint
    }
    pub fn contract_hash(&self) -> &Digest {
        &self.protocol.contract_hash
    }
    pub fn recipe_hash(&self) -> &Digest {
        &self.protocol.recipe_hash
    }
    pub fn limits(&self) -> Limits {
        self.protocol.limits
    }
    pub fn prepare_json(&self, bytes: &[u8]) -> Result<PublicRequest> {
        self.protocol.prepare(bytes, false, None).map(PublicRequest)
    }
    pub fn prepare_stream(&self, bytes: &[u8]) -> Result<PublicRequest> {
        self.protocol.prepare(bytes, true, None).map(PublicRequest)
    }
}
impl Adapter {
    /// Export only public protocol data. Neither credentials, upstream mapping,
    /// connection configuration nor any dispatch capability leaves the provider.
    pub fn public_snapshot(&self) -> PublicAdapterSnapshot {
        PublicAdapterSnapshot {
            version: 1,
            endpoint: self.protocol.endpoint,
            contract: self.protocol.contract.clone(),
            recipe_hash: self.protocol.recipe_hash.clone(),
            limits: self.protocol.limits,
        }
    }
}

/// No body(), conversion to Request, Deref or provider dispatch operation.
///
/// ```compile_fail
/// use mayhem_proxy::endpoint::{PublicRequest, Request};
/// fn dispatchable(request: PublicRequest) -> Request { request }
/// ```
pub struct PublicRequest(Request);
impl PublicRequest {
    /// Verify normalized provisional provider events without exposing a provider
    /// dispatch request. Completion still requires independent final verification.
    pub fn stream(&self, public_id: &str, created: u64) -> Result<PublicStream<'_>> {
        Ok(PublicStream {
            inner: stream::Stream::new(&self.0, public_id, created)?,
            request: self,
            public_id: public_id.into(),
            created,
            failed: false,
        })
    }
    pub fn endpoint(&self) -> ProxyEndpoint {
        self.0.endpoint()
    }
    pub fn request_hash(&self) -> &Digest {
        self.0.request_hash()
    }
    pub fn metering_policy_hash(&self) -> Digest {
        self.0.metering_policy_hash()
    }
    pub fn maximum_usage(&self, output_units: Option<u64>) -> Result<BTreeMap<String, u64>> {
        self.0.maximum_usage(output_units)
    }
    pub fn matches_binding(&self, binding: &crate::attempts::Binding) -> bool {
        self.0.matches_binding(binding)
    }
    pub fn semantic_policy(&self) -> &crate::semantics::Policy {
        self.0.semantic_policy()
    }
    pub fn response_byte_limit(&self) -> usize {
        self.0.limits.response_bytes
    }
    pub fn decode_json(
        &self,
        value: Value,
        public_id: &str,
        created: u64,
    ) -> Result<ProtocolReply> {
        self.0.decode_json(value, public_id, created)
    }
}

/// Bounded normalized-event assembly. Holds current text/tools/items, never an
/// event history. Events are provisional and cannot authorize tools or payment.
pub struct PublicStream<'a> {
    inner: stream::Stream<'a>,
    request: &'a PublicRequest,
    public_id: String,
    created: u64,
    failed: bool,
}
impl PublicStream<'_> {
    pub fn push(&mut self, event: &Value) -> Result<()> {
        require(!self.failed)?;
        self.failed = true;
        let encoded =
            crate::exchange::channel::bounded_json(event, self.request.response_byte_limit())
                .map_err(|_| Error::Protocol)?;
        let frame = crate::worker::Decoded::Sse {
            event: String::new(),
            data: String::from_utf8(encoded).map_err(|_| Error::Protocol)?,
            id: None,
        };
        // Provider normalization is canonical: changed identities, unknown
        // fields, premature terminals and skipped public sequences fail closed.
        let result = self
            .inner
            .push(frame)
            .and_then(|normalized| require(normalized.as_ref() == Some(event)));
        self.failed = result.is_err();
        result
    }
    /// A valid terminal result may supply withheld finish/status metadata, never
    /// replace or extend the observable content already assembled from events.
    pub fn verify_final(self, response: &Value) -> Result<()> {
        require(!self.failed)?;
        crate::exchange::channel::bounded_json(response, self.request.response_byte_limit())
            .map_err(|_| Error::Protocol)?;
        let assembled = self.inner.finish_normalized(response)?;
        let reply = self
            .request
            .decode_json(assembled, &self.public_id, self.created)?;
        require(&reply.body == response)
    }
}
