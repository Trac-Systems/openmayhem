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
}
impl EnrollmentAction {
    fn wire(self) -> &'static str {
        match self {
            Self::Create => "invoice_create",
            Self::Status => "invoice_status",
            Self::Checkout => "invoice_checkout",
        }
    }
    fn path(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Status => "status",
            Self::Checkout => "checkout",
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
    pub original_operation_matches: bool,
    pub authorizes_publication: bool,
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
    pub async fn execute(
        &self,
        key: &SigningKey,
        action: EnrollmentAction,
        rail: Option<mayhem_proto::proxy::ProxyRail>,
    ) -> Result<EnrollmentResult> {
        require(hex(&key.verifying_key().to_bytes()) == self.provider.as_str())?;
        let request = match (action, rail) {
            (EnrollmentAction::Create, Some(mayhem_proto::proxy::ProxyRail::Fiat)) => {
                json!({"rail":"fiat","currency":"usd"})
            }
            (EnrollmentAction::Create, Some(rail)) => json!({"rail":rail}),
            (EnrollmentAction::Status | EnrollmentAction::Checkout, None) => json!({}),
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
            original_operation_matches: false,
            authorizes_publication: false,
        };
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
                }
                let v: Admitted = serde_json::from_value(raw).map_err(|_| Error::Invalid)?;
                require(v.schema_version == 1 && v.purpose == PURPOSE)?;
                report.state = v.state;
                report.entitlement_id = Some(v.entitlement_id);
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
