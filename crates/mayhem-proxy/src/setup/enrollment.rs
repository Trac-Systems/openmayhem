//! Off-ledger provider enrollment. The existing wallet signs only a validated,
//! short-lived identity/action challenge. No transfer, ledger append, model call,
//! generic signing endpoint or automatic publication is performed here.
use super::*;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

const PURPOSE: &str = "proxy_admission_fee";
const AUTH: &str = "mayhem/proxy/admission-auth/v1";
const MAX_RESPONSE: usize = 16384;
const MAX_SAFE: u64 = mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentAction {
    Create,
    Status,
    Checkout,
    Refresh,
    Returns,
}
impl EnrollmentAction {
    fn wire(self) -> &'static str {
        match self {
            Self::Create => "invoice_create",
            Self::Status => "invoice_status",
            Self::Checkout => "invoice_checkout",
            Self::Refresh => "invoice_refresh",
            Self::Returns => "invoice_returns",
        }
    }
    fn path(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Status => "status",
            Self::Checkout => "checkout",
            Self::Refresh => "refresh",
            Self::Returns => "returns",
        }
    }
}

/// The local CLI and dashboard share this client. Bearer grants stay in memory
/// for a single action and never enter stdout, a draft, a URL or a log.
pub struct EnrollmentClient {
    client: reqwest::Client,
    base: url::Url,
    network: Identity,
    provider: Digest,
    operation: Digest,
    timeout_ms: u64,
}
impl Store {
    pub fn enrollment_client(
        &self,
        expected_revision: u64,
        origin: &str,
        timeout_ms: u64,
    ) -> Result<EnrollmentClient> {
        let review = self.inspect()?;
        require(review.revision == expected_revision && review.state == State::StructurallyValid)?;
        let handoff = review.admission_handoff.ok_or(Error::PublicationRecovery)?;
        let (client, base) = admission::peer(origin, timeout_ms)?;
        require(
            base.path() == "/"
                && origin.trim_end_matches('/') == base.origin().ascii_serialization(),
        )?;
        Ok(EnrollmentClient {
            client,
            base,
            network: review.network,
            provider: review.provider_pubkey,
            operation: Digest::new(handoff.initial_operation_digest).map_err(|_| Error::Invalid)?,
            timeout_ms,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    schema_version: u32,
    purpose: String,
    network: Identity,
    public_origin: String,
    provider_pubkey: Digest,
    initial_operation_digest: Digest,
    operation_id: Digest,
    action: String,
    request_digest: Digest,
    client_nonce: Digest,
    challenge_id: Digest,
    nonce: Digest,
    issued_at_ms: u64,
    expires_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeReply {
    schema_version: u32,
    purpose: String,
    signing_domain: String,
    challenge: Challenge,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    schema_version: u32,
    purpose: String,
    network: Identity,
    public_origin: String,
    provider_pubkey: Digest,
    initial_operation_digest: Digest,
    operation_id: Digest,
    action: String,
    request_digest: Digest,
    issued_at_ms: u64,
    expires_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    scheme: String,
    token: String,
    claims: Claims,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantReply {
    schema_version: u32,
    purpose: String,
    authorization: Grant,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentInvoice {
    pub schema_version: u32,
    pub purpose: String,
    pub state: String,
    pub invoice_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoice_commitment: Option<Digest>,
    pub provider_pubkey: Digest,
    pub initial_operation_digest: Digest,
    pub rail: mayhem_proto::proxy::ProxyRail,
    pub payment_status: String,
    pub quote_expires_at_ms: u64,
    pub quote_expired: bool,
    pub fee_usd: String,
    pub amount_base_units: String,
    pub received_amount_base_units: String,
    pub missing_amount_base_units: String,
    pub excess_amount_base_units: String,
    pub collection: Value,
    pub review_code: Option<String>,
    pub permit: Option<EnrollmentPermit>,
    pub publication_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replayed: Option<bool>,
}
/// The operator's exact observed quote. Refresh never silently switches its
/// target to whatever another client most recently created.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentQuote {
    pub invoice_id: Digest,
    pub invoice_commitment: Digest,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentPermit {
    pub body: mayhem_proto::proxy::ProxyAdmissionPermit,
    pub issuer_signature: String,
}

#[derive(Serialize)]
pub struct EnrollmentResult {
    pub schema_version: u32,
    pub kind: &'static str,
    pub state: String,
    pub invoice: Option<EnrollmentInvoice>,
    pub checkout_url: Option<String>,
    pub entitlement_id: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_code: Option<String>,
    pub original_operation_matches: bool,
    pub authorizes_publication: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_page: Option<EnrollmentReturns>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentReturns {
    pub schema_version: u32,
    pub purpose: String,
    pub state: String,
    pub provider_pubkey: Digest,
    pub entries: Vec<EnrollmentReturn>,
    pub next_cursor: Option<String>,
    pub observed_at_ms: u64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentReturn {
    pub refund_id: String,
    pub invoice_id: String,
    pub rail: mayhem_proto::proxy::ProxyRail,
    pub amount_base_units: String,
    pub state: String,
    pub reason: String,
    pub destination: Value,
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub next_attempt_at_ms: Option<u64>,
    pub action_required: String,
}
impl EnrollmentReturns {
    fn validate(&self, provider: &Digest) -> Result<()> {
        require(self.schema_version == 1 && self.purpose == PURPOSE && self.state == "returns"
            && &self.provider_pubkey == provider && self.entries.len() <= 16
            && self.observed_at_ms > 0 && self.observed_at_ms <= MAX_SAFE)?;
        if let Some(cursor) = &self.next_cursor {
            require(self.entries.len() == 16 && self.entries.last().is_some_and(|r| &r.refund_id == cursor))?;
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.entries {
            require(safe_id(&r.refund_id,128) && safe_id(&r.invoice_id,128)
                && seen.insert(&r.refund_id) && amount(&r.amount_base_units)? > 0
                && r.created_at_ms > 0 && r.created_at_ms <= MAX_SAFE
                && r.completed_at_ms.is_none_or(|v| v > 0 && v <= MAX_SAFE)
                && r.next_attempt_at_ms.is_none_or(|v| v > 0 && v <= MAX_SAFE)
                && matches!(r.state.as_str(),"queued"|"confirming"|"review"|"returned")
                && matches!(r.reason.as_str(),"queued"|"confirming"|"review_required"|"returned"|"funding_required"|"gas_funding_required"|"fee_limit"|"approval_expired"|"verification_unavailable")
                && matches!(r.action_required.as_str(),"operator"|"none")
                && (r.state == "returned") == r.completed_at_ms.is_some()
                && (!matches!(r.state.as_str(),"returned"|"review") || r.next_attempt_at_ms.is_none()))?;
            let expected_action = if r.state == "review" || matches!(r.reason.as_str(), "funding_required"|"gas_funding_required"|"fee_limit"|"approval_expired") { "operator" } else { "none" };
            require(r.action_required == expected_action
                && (r.state == "returned") == (r.reason == "returned")
                && (!matches!(r.reason.as_str(), "queued"|"confirming") || r.reason == r.state)
                && (r.reason != "review_required" || r.state == "review"))?;
            match r.rail {
                mayhem_proto::proxy::ProxyRail::Fiat => {
                    #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct D { kind:String, currency:String }
                    let d:D=serde_json::from_value(r.destination.clone()).map_err(|_|Error::Invalid)?;
                    require(d.kind=="original_payment_method" && d.currency.len()==3 && d.currency.bytes().all(|b|b.is_ascii_lowercase()))?;
                }
                mayhem_proto::proxy::ProxyRail::Tnk => {
                    #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct D { kind:String, network:String, address:String }
                    let d:D=serde_json::from_value(r.destination.clone()).map_err(|_|Error::Invalid)?;
                    require(d.kind=="crypto_address" && matches!(d.network.as_str(),"mainnet"|"testnet1") && safe_id(&d.address,128)
                        && d.address.starts_with(if d.network=="mainnet" {"trac1"} else {"testtrac1"}))?;
                }
                mayhem_proto::proxy::ProxyRail::Tap => {
                    #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct D {kind:String,chain_id:u64,token_contract:String,address:String}
                    let d:D=serde_json::from_value(r.destination.clone()).map_err(|_|Error::Invalid)?;
                    let eth=|v:&str| v.len()==42 && v.starts_with("0x") && v[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                    require(d.kind=="crypto_address" && d.chain_id>0 && d.chain_id<=MAX_SAFE && eth(&d.token_contract) && eth(&d.address))?;
                }
            }
        }
        Ok(())
    }
}

fn signing_bytes<T: Serialize>(domain: &str, value: &T) -> Result<Vec<u8>> {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend(
        mayhem_proto::stable_json_bytes(&serde_json::to_value(value).map_err(|_| Error::Invalid)?)
            .map_err(|_| Error::Invalid)?,
    );
    require(bytes.len() <= MAX_RESPONSE)?;
    Ok(bytes)
}
fn digest<T: Serialize>(domain: &str, value: &T) -> Result<Digest> {
    Digest::new(
        blake3::hash(&signing_bytes(domain, value)?)
            .to_hex()
            .to_string(),
    )
    .map_err(|_| Error::Invalid)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Invalid)?
        .as_millis()
        .try_into()
        .map_err(|_| Error::Invalid)?)
}
fn valid_time(issued: u64, expires: u64) -> Result<()> {
    let current = now()?;
    require(
        issued <= current.saturating_add(60000)
            && current < expires
            && expires <= MAX_SAFE
            && expires > issued
            && expires - issued <= 600000,
    )
}
fn amount(value: &str) -> Result<u128> {
    require(
        !value.is_empty()
            && value.len() <= 39
            && value.bytes().all(|b| b.is_ascii_digit())
            && (value == "0" || !value.starts_with('0')),
    )?;
    value.parse().map_err(|_| Error::Invalid)
}
fn safe_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl EnrollmentClient {
    pub async fn returns(&self,key:&SigningKey,after:Option<String>) -> Result<EnrollmentResult> {
        require(hex(&key.verifying_key().to_bytes())==self.provider.as_str())?;
        let request=if let Some(after)=after { require(safe_id(&after,128))?;json!({"after":after}) } else {json!({})};
        tokio::time::timeout(Duration::from_millis(self.timeout_ms), self.perform(key,EnrollmentAction::Returns,request))
            .await.map_err(|_|Error::EnrollmentUnavailable)?
    }
    pub async fn execute(
        &self,
        key: &SigningKey,
        action: EnrollmentAction,
        rail: Option<mayhem_proto::proxy::ProxyRail>,
    ) -> Result<EnrollmentResult> {
        self.execute_with_quote(key, action, rail, None).await
    }
    pub async fn execute_with_quote(
        &self,
        key: &SigningKey,
        action: EnrollmentAction,
        rail: Option<mayhem_proto::proxy::ProxyRail>,
        quote: Option<EnrollmentQuote>,
    ) -> Result<EnrollmentResult> {
        require(hex(&key.verifying_key().to_bytes()) == self.provider.as_str())?;
        require(matches!(action, EnrollmentAction::Refresh) == quote.is_some())?;
        let request = match (action, rail) {
            (EnrollmentAction::Create, Some(mayhem_proto::proxy::ProxyRail::Fiat)) => {
                json!({"rail":"fiat","currency":"usd"})
            }
            (EnrollmentAction::Create, Some(rail)) => json!({"rail":rail}),
            (EnrollmentAction::Status | EnrollmentAction::Checkout | EnrollmentAction::Returns, None) => json!({}),
            (EnrollmentAction::Refresh, None) => {
                serde_json::to_value(quote.ok_or(Error::Invalid)?).map_err(|_| Error::Invalid)?
            }
            _ => return Err(Error::Invalid),
        };
        // One bounded operation, including challenge, exchange and invoice I/O.
        // Retrying a later invocation uses the server's original provider/network
        // invoice identity; a timeout never authorizes another fee.
        tokio::time::timeout(
            Duration::from_millis(self.timeout_ms),
            self.perform(key, action, request),
        )
        .await
        .map_err(|_| Error::EnrollmentUnavailable)?
    }
    async fn post(&self, path: &str, body: &Value, authorization: Option<&str>) -> Result<Vec<u8>> {
        let bytes = serde_json::to_vec(body).map_err(|_| Error::Invalid)?;
        require(bytes.len() <= 4096)?;
        let mut request = self
            .client
            .post(self.base.join(path).map_err(|_| Error::Invalid)?)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes);
        if let Some(token) = authorization {
            let mut header =
                reqwest::header::HeaderValue::from_str(&format!("ProxyAdmission {token}"))
                    .map_err(|_| Error::Invalid)?;
            header.set_sensitive(true);
            request = request.header(reqwest::header::AUTHORIZATION, header);
        }
        let response = request
            .send()
            .await
            .map_err(|_| Error::EnrollmentUnavailable)?;
        admission::bounded_response(response, MAX_RESPONSE)
            .await
            .map_err(|_| Error::EnrollmentUnavailable)
    }
    async fn perform(
        &self,
        key: &SigningKey,
        action: EnrollmentAction,
        request: Value,
    ) -> Result<EnrollmentResult> {
        let public_origin = self.base.origin().ascii_serialization();
        let operation_id = digest(
            "mayhem/proxy/admission-operation/v1",
            &json!({"network":self.network,"public_origin":public_origin,
            "provider_pubkey":self.provider,"initial_operation_digest":self.operation}),
        )?;
        let request_digest = digest(
            "mayhem/proxy/admission-request/v1",
            &json!({"action":action.wire(),"request":request}),
        )?;
        let mut nonce = [0; 32];
        getrandom::fill(&mut nonce).map_err(|_| Error::Storage)?;
        let client_nonce = Digest::new(hex(&nonce)).map_err(|_| Error::Invalid)?;
        let bytes = self.post("v1/proxy/admission/auth/challenge", &json!({"schema_version":1,"provider_pubkey":self.provider,
            "initial_operation_digest":self.operation,"action":action.wire(),"request_digest":request_digest,"client_nonce":client_nonce}),None).await?;
        let reply: ChallengeReply = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        let c = reply.challenge;
        require(
            reply.schema_version == 1
                && reply.purpose == PURPOSE
                && reply.signing_domain == AUTH
                && c.schema_version == 1
                && c.purpose == PURPOSE
                && c.public_origin == public_origin
                && c.network == self.network
                && c.provider_pubkey == self.provider
                && c.initial_operation_digest == self.operation
                && c.operation_id == operation_id
                && c.request_digest == request_digest
                && c.action == action.wire()
                && c.client_nonce == client_nonce,
        )?;
        require(
            c.challenge_id
                == digest(
                    "mayhem/proxy/admission-challenge-id/v1",
                    &json!({"network":self.network,"public_origin":public_origin,
            "provider_pubkey":self.provider,"client_nonce":client_nonce}),
                )?,
        )?;
        valid_time(c.issued_at_ms, c.expires_at_ms)?;
        let signature = hex(&key.sign(&signing_bytes(AUTH, &c)?).to_bytes());
        let response = Zeroizing::new(
            self.post(
                "v1/proxy/admission/auth/verify",
                &json!({"schema_version":1,"challenge_id":c.challenge_id,
            "provider_signature":signature}),
                None,
            )
            .await?,
        );
        let grant: GrantReply = serde_json::from_slice(&response).map_err(|_| Error::Invalid)?;
        let token = Zeroizing::new(grant.authorization.token);
        let claims = grant.authorization.claims;
        require(
            grant.schema_version == 1
                && grant.purpose == PURPOSE
                && grant.authorization.scheme == "ProxyAdmission"
                && claims.schema_version == 1
                && claims.purpose == PURPOSE
                && claims.network == self.network
                && claims.public_origin == public_origin
                && claims.provider_pubkey == self.provider
                && claims.initial_operation_digest == self.operation
                && claims.operation_id == operation_id
                && claims.request_digest == request_digest
                && claims.action == action.wire()
                && Digest::new(token.as_str()).is_ok(),
        )?;
        valid_time(claims.issued_at_ms, claims.expires_at_ms)?;
        let result = self
            .post(
                &format!("v1/proxy/admission/invoice/{}", action.path()),
                &json!({"schema_version":1,"operation_id":operation_id,"request":request}),
                Some(&token),
            )
            .await?;
        self.result(action, &result)
    }
    fn result(&self, action: EnrollmentAction, bytes: &[u8]) -> Result<EnrollmentResult> {
        let raw: Value = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
        let mut report = EnrollmentResult {
            schema_version: 1,
            kind: "proxy_admission",
            state: String::new(),
            invoice: None,
            checkout_url: None,
            entitlement_id: None,
            review_code: None,
            original_operation_matches: false,
            authorizes_publication: false,
            return_page: None,
        };
        if matches!(action,EnrollmentAction::Returns) {
            let page:EnrollmentReturns=serde_json::from_value(raw).map_err(|_|Error::Invalid)?;
            page.validate(&self.provider)?;
            report.state="returns".into();report.return_page=Some(page);return Ok(report);
        }
        if matches!(action, EnrollmentAction::Checkout) {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Checkout {
                state: String,
                url: Option<String>,
            }
            let checkout: Checkout = serde_json::from_value(raw).map_err(|_| Error::Invalid)?;
            require(matches!(
                checkout.state.as_str(),
                "checkout" | "settling" | "expired"
            ))?;
            if let Some(url) = &checkout.url {
                let u = url::Url::parse(url).map_err(|_| Error::Invalid)?;
                require(
                    checkout.state == "checkout"
                        && url.len() <= 4096
                        && u.scheme() == "https"
                        && u.host_str() == Some("checkout.stripe.com")
                        && u.username().is_empty()
                        && u.password().is_none()
                        && u.port_or_known_default() == Some(443),
                )?;
            }
            require(checkout.state != "checkout" || checkout.url.is_some())?;
            report.state = checkout.state;
            report.checkout_url = checkout.url;
            return Ok(report);
        }
        match raw.get("state").and_then(Value::as_str) {
            Some("invoice") => {
                let invoice: EnrollmentInvoice =
                    serde_json::from_value(raw).map_err(|_| Error::Invalid)?;
                invoice.validate(&self.provider, &self.network)?;
                report.original_operation_matches =
                    invoice.initial_operation_digest == self.operation;
                report.state = invoice.payment_status.clone();
                report.review_code = invoice.review_code.clone();
                report.invoice = Some(invoice);
            }
            Some("admitted") => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Admitted {
                    schema_version: u32,
                    purpose: String,
                    state: String,
                    entitlement_id: Digest,
                    review_code: Option<String>,
                }
                let v: Admitted = serde_json::from_value(raw).map_err(|_| Error::Invalid)?;
                require(v.schema_version == 1 && v.purpose == PURPOSE)?;
                require(v.review_code.as_ref().is_none_or(|code| {
                    !code.is_empty()
                        && code.len() <= 100
                        && code
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                }))?;
                report.state = v.state;
                report.entitlement_id = Some(v.entitlement_id);
                report.review_code = v.review_code;
            }
            Some("no_local_invoice") => {
                require(
                    matches!(action, EnrollmentAction::Status)
                        && raw
                            == json!({"schema_version":1,"purpose":PURPOSE,"state":"no_local_invoice","publication_status":"not_checked"}),
                )?;
                report.state = "no_local_invoice".into();
            }
            _ => return Err(Error::Invalid),
        }
        Ok(report)
    }
}
impl EnrollmentInvoice {
    fn validate(&self, provider: &Digest, network: &Identity) -> Result<()> {
        require(
            self.schema_version == 1
                && self.purpose == PURPOSE
                && self.state == "invoice"
                && &self.provider_pubkey == provider
                && self.fee_usd == "10.00"
                && self.publication_status == "not_checked"
                && safe_id(&self.invoice_id, 128)
                && (1..=MAX_SAFE).contains(&self.quote_expires_at_ms)
                && matches!(
                    self.payment_status.as_str(),
                    "awaiting_payment"
                        | "confirming"
                        | "short_payment"
                        | "issuing"
                        | "ready"
                        | "review"
                )
                && self.review_code.as_ref().is_none_or(|v| safe_id(v, 100)),
        )?;
        let expected = amount(&self.amount_base_units)?;
        let received = amount(&self.received_amount_base_units)?;
        require(
            expected > 0
                && amount(&self.missing_amount_base_units)? == expected.saturating_sub(received)
                && amount(&self.excess_amount_base_units)? == received.saturating_sub(expected),
        )?;
        match self.rail {
            mayhem_proto::proxy::ProxyRail::Fiat => {
                require(self.collection == json!({"currency":"usd"}) && expected == 1000)?
            }
            mayhem_proto::proxy::ProxyRail::Tnk => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Tnk {
                    network: String,
                    destination: String,
                }
                let c: Tnk =
                    serde_json::from_value(self.collection.clone()).map_err(|_| Error::Invalid)?;
                require(
                    matches!(c.network.as_str(), "mainnet" | "testnet1")
                        && safe_id(&c.destination, 128)
                        && c.destination.starts_with(if c.network == "mainnet" {
                            "trac1"
                        } else {
                            "testtrac1"
                        }),
                )?;
            }
            mayhem_proto::proxy::ProxyRail::Tap => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Tap {
                    chain_id: u64,
                    token_contract: String,
                    destination: String,
                }
                let c: Tap =
                    serde_json::from_value(self.collection.clone()).map_err(|_| Error::Invalid)?;
                let address = |v: &str| {
                    v.len() == 42
                        && v.starts_with("0x")
                        && v[2..]
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                };
                require(
                    (1..=MAX_SAFE).contains(&c.chain_id)
                        && address(&c.token_contract)
                        && address(&c.destination),
                )?;
            }
        }
        if let Some(signed) = &self.permit {
            let p = &signed.body;
            let bytes = p.signing_bytes().map_err(|_| Error::Invalid)?;
            require(
                self.payment_status == "ready"
                    && p.provider_pubkey == provider.as_str()
                    && p.initial_operation_digest == self.initial_operation_digest.as_str()
                    && p.network_id == network.network_id
                    && p.msb_bootstrap == network.msb_bootstrap
                    && p.subnet_bootstrap == network.subnet_bootstrap
                    && p.rail == self.rail
                    && p.accepted_amount == expected
                    && p.accepted_value_au == 10_000_000_000_000_000_000
                    && crate::receipts::verify_signature(
                        &signed.issuer_signature,
                        &bytes,
                        &p.issuer_pubkey,
                    ),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod admitted_review_tests {
    use super::*;

    #[test]
    fn return_pages_reject_inconsistent_or_private_payloads_and_fit_response_budget() {
        let provider = Digest::new("33".repeat(32)).unwrap();
        let base = json!({"schema_version":1,"purpose":PURPOSE,"state":"returns","provider_pubkey":provider,
            "entries":[{"refund_id":"r1","invoice_id":"i1","rail":"fiat","amount_base_units":"125","state":"queued","reason":"funding_required",
            "destination":{"kind":"original_payment_method","currency":"usd"},"created_at_ms":100,"completed_at_ms":null,"next_attempt_at_ms":1000,"action_required":"operator"}],
            "next_cursor":null,"observed_at_ms":1000});
        let validate = |value:Value| serde_json::from_value::<EnrollmentReturns>(value).map_err(|_|Error::Invalid).and_then(|p|p.validate(&provider));
        assert!(validate(base.clone()).is_ok());
        for (rail,destination) in [
            ("fiat", json!({"kind":"original_payment_method","currency":"eur"})),
            ("tnk", json!({"kind":"crypto_address","network":"testnet1","address":format!("testtrac1{}","a".repeat(60))})),
            ("tap", json!({"kind":"crypto_address","chain_id":1,"token_contract":format!("0x{}","a".repeat(40)),"address":format!("0x{}","b".repeat(40))})),
        ] {
            let mut p=base.clone(); p["entries"][0]["rail"]=json!(rail);p["entries"][0]["destination"]=destination;
            for (state,reason,action) in [("queued","queued","none"),("confirming","confirming","none"),("review","review_required","operator"),("returned","returned","none")] {
                p["entries"][0]["state"]=json!(state);p["entries"][0]["reason"]=json!(reason);p["entries"][0]["action_required"]=json!(action);
                p["entries"][0]["completed_at_ms"]=if state=="returned" {json!(1000)} else {Value::Null};
                p["entries"][0]["next_attempt_at_ms"]=if matches!(state,"review"|"returned") {Value::Null} else {json!(1000)};
                assert!(validate(p.clone()).is_ok());
            }
            p["entries"][0]["destination"]["private_signature"]=json!("must not escape");assert!(validate(p).is_err());
        }
        for (field,value) in [("state",json!("delivered")),("amount_base_units",json!("0")),("amount_base_units",json!("1.5")),("refund_id",json!("../escape")),("reason",json!("raw_processor_error")),("reason",json!("returned")),("action_required",json!("none")),("completed_at_ms",json!(10)),("created_at_ms",json!(MAX_SAFE+1))] {
            let mut p=base.clone();p["entries"][0][field]=value;assert!(validate(p).is_err(),"{field}");
        }
        let mut p=base.clone();p["provider_pubkey"]=json!("44".repeat(32));assert!(validate(p).is_err());
        let mut p=base.clone();p["next_cursor"]=json!("r1");assert!(validate(p).is_err());
        let mut p=base.clone();p["entries"]=json!([base["entries"][0],base["entries"][0]]);assert!(validate(p).is_err());
        let mut p=base.clone();p["entries"]=json!((0..16).map(|n|{let mut e=base["entries"][0].clone();e["refund_id"]=json!(format!("{:0>128}",n));e["invoice_id"]=json!("i".repeat(128));e["amount_base_units"]=json!(u128::MAX.to_string());e["rail"]=json!("tnk");e["destination"]=json!({"kind":"crypto_address","network":"testnet1","address":format!("testtrac1{}","a".repeat(119))});e}).collect::<Vec<_>>());
        p["next_cursor"]=p["entries"][15]["refund_id"].clone();assert!(validate(p.clone()).is_ok());
        assert!(serde_json::to_vec(&p).unwrap().len()<16*1024);
        p["entries"].as_array_mut().unwrap().push(base["entries"][0].clone());assert!(validate(p).is_err());
    }

    #[test]
    fn admitted_review_preserves_entitlement_without_authorizing_publication() {
        let client = EnrollmentClient {
            client: reqwest::Client::new(),
            base: url::Url::parse("https://admission.invalid").unwrap(),
            network: Identity {
                network_id: "test".into(),
                msb_bootstrap: "11".repeat(32),
                subnet_bootstrap: "22".repeat(32),
                contract_version: 30,
            },
            provider: Digest::new("33".repeat(32)).unwrap(),
            operation: Digest::new("44".repeat(32)).unwrap(),
            timeout_ms: 1000,
        };
        let admitted = json!({"schema_version":1,"purpose":PURPOSE,"state":"admitted","entitlement_id":"55".repeat(32)});
        for action in [EnrollmentAction::Create, EnrollmentAction::Status] {
            for review in [None, Some("stripe_reversal_review")] {
                let mut input = admitted.clone();
                if let Some(code) = review {
                    input["review_code"] = json!(code);
                }
                let report = client
                    .result(action, &serde_json::to_vec(&input).unwrap())
                    .unwrap();
                assert_eq!(report.state, "admitted");
                assert_eq!(
                    report.entitlement_id.as_ref().unwrap().as_str(),
                    "55".repeat(32)
                );
                assert_eq!(report.review_code.as_deref(), review);
                assert!(report.invoice.is_none() && report.checkout_url.is_none());
                assert!(!report.authorizes_publication && !report.original_operation_matches);
                let wire = serde_json::to_value(&report).unwrap();
                assert_eq!(wire.get("review_code"), input.get("review_code"));
            }
        }
        for code in [
            json!(""),
            json!("x".repeat(101)),
            json!("<script>"),
            json!("UPPER"),
            json!(42),
        ] {
            let mut input = admitted.clone();
            input["review_code"] = code;
            assert!(client
                .result(
                    EnrollmentAction::Status,
                    &serde_json::to_vec(&input).unwrap()
                )
                .is_err());
        }
        let mut unexpected = admitted;
        unexpected["extra"] = json!(true);
        assert!(client
            .result(
                EnrollmentAction::Status,
                &serde_json::to_vec(&unexpected).unwrap()
            )
            .is_err());
    }
}
