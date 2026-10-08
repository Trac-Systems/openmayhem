//! Independently reproducible, versioned proxy quantities. Native tokenizer usage,
//! cache claims and opaque reasoning are telemetry, never billing evidence here.
//! An observation/subtotal is NOT a receipt, authorization or settlement action.

use crate::{
    attempts::{Binding, Digest, Recovery},
    endpoint,
};
use mayhem_proto::{
    metered_output_units, normalized_request_prompt_units,
    proxy::{ProxyEndpoint, ProxyMeteringContract},
    MoneyAu, VisibleToolCall,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fmt};

// Existing native helpers implement this policy's numerical unit. A future
// change must add a new policy, not silently reinterpret accepted v1 offers.
const _: () = assert!(mayhem_proto::NORMALIZED_REQUEST_BYTES_PER_PROMPT_UNIT == 4);
const _: () = assert!(mayhem_proto::VISIBLE_OUTPUT_BYTES_PER_UNIT == 4);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the accepted proxy metering policy is unsupported or mismatched")]
    Policy,
    #[error("this input needs a metering policy for its non-text or unsupported content")]
    UnsupportedInput,
    #[error("proxy metering evidence does not match the accepted request/result")]
    Evidence,
    #[error("partial, refused or cancelled work needs an explicit accepted charging policy")]
    OutcomePolicyRequired,
    #[error("the retained proxy offer does not match the accepted binding")]
    Offer,
    #[error("proxy subtotal exceeds the remaining accepted authorization")]
    Budget,
    #[error("proxy metering arithmetic overflow")]
    Overflow,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Policy {
    ObservableTextV1,
    CompletedDecisionsV1,
}
impl Policy {
    pub fn for_endpoint(endpoint: ProxyEndpoint) -> Self {
        if endpoint == ProxyEndpoint::Decisions {
            Self::CompletedDecisionsV1
        } else {
            Self::ObservableTextV1
        }
    }
    /// Public, immutable algorithm description. A change requires a new policy
    /// version/hash, not reinterpretation of already accepted quantities.
    pub fn definition(self) -> Value {
        match self {
            Self::ObservableTextV1 => json!({
                "version":1,"algorithm":"observable_text_v1",
                "units":["input_token","output_token"],
                "unit_meaning":"normalized billing units, not native tokenizer tokens",
                "input":"normalized_request_prompt_units: ceil(stable JSON projection UTF-8 bytes / 4), once per request",
                "chat_fields":["messages","tools","response_format"],
                "completions_fields":["prompt","suffix"],
                "responses_fields":["input","instructions","tools","text"],
                "input_rules":"text-only documented message/history parts; no opaque input state, token IDs, media or file references; absent/null optional root fields omitted; object keys sorted, array order retained",
                "output":"metered_output_units: ceil((all text + refusal + observable reasoning UTF-8 bytes + stable JSON of VisibleToolCall array) / 4), once across all choices/items",
                "reasoning":"equal chat aliases counted once, conflicting aliases rejected; Responses content and summary counted, encrypted_content excluded",
                "excluded":"upstream usage, cache claims, hidden reasoning counts, timestamps, model/response/item IDs except tool call IDs, annotations, logprobs, transport framing",
                "outcomes":"quantities are observations; partial/refused/cancelled charging requires separate accepted policy",
                "cache":"no claimed cache discount"
            }),
            Self::CompletedDecisionsV1 => json!({
                "version":1,"algorithm":"completed_decisions_v1","units":["decision"],
                "input":"bounded typed decision state/questions under accepted endpoint contract",
                "output":"one unit for each requested question with a validated corresponding answer",
                "excluded":"upstream tokens, reasoning, cache, steps, elapsed time, probabilities as compute quantities",
                "outcomes":"incomplete or invalid question/answer sets cannot be settled as completed decisions"
            }),
        }
    }
    pub fn hash(self) -> Digest {
        endpoint::digest("mayhem/proxy/metering-policy/v1", &self.definition())
            .expect("fixed metering definition")
    }
    pub fn contract(self) -> ProxyMeteringContract {
        ProxyMeteringContract {
            policy_hash: self.hash().as_str().into(),
            units: self.units().iter().map(|s| (*s).into()).collect(),
        }
    }
    fn units(self) -> &'static [&'static str] {
        match self {
            Self::ObservableTextV1 => &["input_token", "output_token"],
            Self::CompletedDecisionsV1 => &["decision"],
        }
    }
    pub fn resolve(endpoint: ProxyEndpoint, hash: &Digest) -> Result<Self> {
        let policy = Self::for_endpoint(endpoint);
        if &policy.hash() != hash {
            return Err(Error::Policy);
        }
        Ok(policy)
    }
    pub fn prepare(self, endpoint: ProxyEndpoint, request: &Value) -> Result<Prepared> {
        if self != Self::for_endpoint(endpoint) || !request.is_object() {
            return Err(Error::Policy);
        }
        let input_units = if self == Self::ObservableTextV1 {
            let projection = project_input(endpoint, request)?;
            normalized_request_prompt_units(&projection).map_err(|_| Error::Evidence)?
        } else {
            0
        };
        let questions = request
            .get("questions")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        Ok(Prepared {
            policy: self,
            endpoint,
            request_hash: Digest::new(mayhem_proto::endpoint_request_fingerprint(request))
                .map_err(|_| Error::Evidence)?,
            input_units,
            questions,
        })
    }
}

pub struct Prepared {
    policy: Policy,
    endpoint: ProxyEndpoint,
    request_hash: Digest,
    input_units: u64,
    questions: Vec<String>,
}
impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedMetering")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Complete,
    Incomplete,
    Refused,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub policy_hash: Digest,
    pub request_hash: Digest,
    pub result_hash: Digest,
    pub disposition: Disposition,
    pub units: BTreeMap<String, u64>,
}
impl Prepared {
    pub fn policy_hash(&self) -> Digest {
        self.policy.hash()
    }
    /// Only normalized, endpoint/schema-validated output belongs here. This
    /// independently counts its evidence; it does not validate model intelligence.
    pub fn observe(&self, result: &Value) -> Result<Observation> {
        let (units, disposition) = if self.policy == Policy::CompletedDecisionsV1 {
            let answers = result
                .get("answers")
                .and_then(Value::as_object)
                .ok_or(Error::Evidence)?;
            if self.questions.is_empty()
                || answers.len() != self.questions.len()
                || self.questions.iter().any(|q| !answers.contains_key(q))
            {
                return Err(Error::Evidence);
            }
            (
                BTreeMap::from([("decision".into(), answers.len() as u64)]),
                Disposition::Complete,
            )
        } else {
            let (output, disposition) = output_units(self.endpoint, result)?;
            (
                BTreeMap::from([
                    ("input_token".into(), self.input_units),
                    ("output_token".into(), output),
                ]),
                disposition,
            )
        };
        if units
            .values()
            .any(|n| *n > mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
        {
            return Err(Error::Overflow);
        }
        Ok(Observation {
            policy_hash: self.policy.hash(),
            request_hash: self.request_hash.clone(),
            result_hash: endpoint::digest("mayhem/proxy/metered-result/v1", result)
                .map_err(|_| Error::Evidence)?,
            disposition,
            units,
        })
    }
}

fn fields(value: &Value, allowed: &[&str]) -> Result<()> {
    let map = value.as_object().ok_or(Error::UnsupportedInput)?;
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(Error::UnsupportedInput);
    }
    Ok(())
}
fn text_parts(value: &Value, responses: bool) -> Result<()> {
    if value.is_string() || value.is_null() {
        return Ok(());
    }
    for part in value.as_array().ok_or(Error::UnsupportedInput)? {
        let kind = part
            .get("type")
            .and_then(Value::as_str)
            .ok_or(Error::UnsupportedInput)?;
        let field = if responses && kind == "refusal" {
            "refusal"
        } else {
            "text"
        };
        if !(kind == "text"
            || (responses
                && matches!(
                    kind,
                    "input_text" | "output_text" | "refusal" | "reasoning_text" | "summary_text"
                )))
        {
            return Err(Error::UnsupportedInput);
        }
        fields(part, &["type", field])?;
        if !part.get(field).is_some_and(Value::is_string) {
            return Err(Error::UnsupportedInput);
        }
    }
    Ok(())
}
fn text_field(value: &Value, key: &str) -> Result<()> {
    if value
        .get(key)
        .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        Err(Error::UnsupportedInput)
    } else {
        Ok(())
    }
}
fn chat_message(value: &Value) -> Result<()> {
    fields(
        value,
        &[
            "role",
            "content",
            "name",
            "tool_call_id",
            "tool_calls",
            "refusal",
            "reasoning",
            "reasoning_content",
        ],
    )?;
    if !value
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|r| matches!(r, "system" | "developer" | "user" | "assistant" | "tool"))
    {
        return Err(Error::UnsupportedInput);
    }
    if let Some(content) = value.get("content") {
        text_parts(content, false)?;
    }
    for field in [
        "name",
        "tool_call_id",
        "refusal",
        "reasoning",
        "reasoning_content",
    ] {
        text_field(value, field)?;
    }
    if let Some(calls) = value.get("tool_calls").filter(|v| !v.is_null()) {
        for c in calls.as_array().ok_or(Error::UnsupportedInput)? {
            fields(c, &["id", "type", "function"])?;
            if c.get("type").and_then(Value::as_str) != Some("function") {
                return Err(Error::UnsupportedInput);
            }
            fields(&c["function"], &["name", "arguments"])?;
            for (obj, key) in [
                (c, "id"),
                (&c["function"], "name"),
                (&c["function"], "arguments"),
            ] {
                if !obj.get(key).is_some_and(Value::is_string) {
                    return Err(Error::UnsupportedInput);
                }
            }
        }
    }
    Ok(())
}
fn responses_input(value: &Value) -> Result<()> {
    if value.is_string() {
        return Ok(());
    }
    for item in value.as_array().ok_or(Error::UnsupportedInput)? {
        match item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message")
        {
            "message" => {
                fields(item, &["type", "role", "content", "id", "status"])?;
                if !item
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|r| matches!(r, "user" | "assistant" | "system" | "developer"))
                {
                    return Err(Error::UnsupportedInput);
                }
                text_parts(item.get("content").ok_or(Error::UnsupportedInput)?, true)?;
            }
            "function_call" => {
                fields(
                    item,
                    &["type", "id", "call_id", "name", "arguments", "status"],
                )?;
                for key in ["name", "arguments", "call_id"] {
                    if !item.get(key).is_some_and(Value::is_string) {
                        return Err(Error::UnsupportedInput);
                    }
                }
            }
            "function_call_output" => {
                fields(item, &["type", "id", "call_id", "output", "status"])?;
                if !item.get("call_id").is_some_and(Value::is_string) {
                    return Err(Error::UnsupportedInput);
                }
                text_parts(item.get("output").ok_or(Error::UnsupportedInput)?, true)?;
            }
            "reasoning" => {
                fields(item, &["type", "id", "summary", "content", "status"])?;
                for key in ["content", "summary"] {
                    if let Some(v) = item.get(key) {
                        text_parts(v, true)?;
                    }
                }
            }
            _ => return Err(Error::UnsupportedInput),
        }
    }
    Ok(())
}
fn project_input(endpoint: ProxyEndpoint, value: &Value) -> Result<Value> {
    let names: &[&str] = match endpoint {
        ProxyEndpoint::Chat => {
            for msg in value
                .get("messages")
                .and_then(Value::as_array)
                .ok_or(Error::UnsupportedInput)?
            {
                chat_message(msg)?;
            }
            &["messages", "tools", "response_format"]
        }
        ProxyEndpoint::Completions => {
            let p = value.get("prompt").ok_or(Error::UnsupportedInput)?;
            if !(p.is_string()
                || p.as_array()
                    .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string)))
            {
                return Err(Error::UnsupportedInput);
            }
            text_field(value, "suffix")?;
            &["prompt", "suffix"]
        }
        ProxyEndpoint::Responses => {
            responses_input(value.get("input").ok_or(Error::UnsupportedInput)?)?;
            text_field(value, "instructions")?;
            &["input", "instructions", "tools", "text"]
        }
        _ => return Err(Error::Policy),
    };
    let mut projected = Map::new();
    for name in names {
        if let Some(v) = value.get(*name).filter(|v| !v.is_null()) {
            projected.insert((*name).into(), v.clone());
        }
    }
    Ok(Value::Object(projected))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(Error::Evidence)
}
fn optional_text(value: &Value, key: &str, out: &mut String) -> Result<()> {
    if let Some(v) = value.get(key).filter(|v| !v.is_null()) {
        out.push_str(v.as_str().ok_or(Error::Evidence)?);
    }
    Ok(())
}
fn output_units(endpoint: ProxyEndpoint, value: &Value) -> Result<(u64, Disposition)> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    let mut incomplete = false;
    let mut refused = false;
    match endpoint {
        ProxyEndpoint::Chat | ProxyEndpoint::Completions => {
            for choice in value
                .get("choices")
                .and_then(Value::as_array)
                .ok_or(Error::Evidence)?
            {
                incomplete |= string(choice, "finish_reason")? == "length";
                refused |= string(choice, "finish_reason")? == "content_filter";
                if endpoint == ProxyEndpoint::Completions {
                    text.push_str(string(choice, "text")?);
                    continue;
                }
                let m = &choice["message"];
                optional_text(m, "content", &mut text)?;
                optional_text(m, "refusal", &mut text)?;
                refused |= m
                    .get("refusal")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty());
                let a = m.get("reasoning_content").filter(|v| !v.is_null());
                let b = m.get("reasoning").filter(|v| !v.is_null());
                if a.is_some() && b.is_some() && a != b {
                    return Err(Error::Evidence);
                }
                if let Some(v) = a.or(b) {
                    reasoning.push_str(v.as_str().ok_or(Error::Evidence)?);
                }
                if let Some(raw) = m.get("tool_calls").filter(|v| !v.is_null()) {
                    for c in raw.as_array().ok_or(Error::Evidence)? {
                        calls.push(VisibleToolCall {
                            id: string(c, "id")?.into(),
                            name: string(&c["function"], "name")?.into(),
                            arguments: string(&c["function"], "arguments")?.into(),
                        });
                    }
                }
            }
        }
        ProxyEndpoint::Responses => {
            incomplete = string(value, "status")? == "incomplete";
            for item in value
                .get("output")
                .and_then(Value::as_array)
                .ok_or(Error::Evidence)?
            {
                match string(item, "type")? {
                    "function_call" => calls.push(VisibleToolCall {
                        id: string(item, "call_id")?.into(),
                        name: string(item, "name")?.into(),
                        arguments: string(item, "arguments")?.into(),
                    }),
                    kind @ ("message" | "reasoning") => {
                        for field in ["content", "summary"] {
                            if let Some(parts) = item.get(field) {
                                for part in parts.as_array().ok_or(Error::Evidence)? {
                                    let refusal = string(part, "type")? == "refusal";
                                    refused |= refusal;
                                    let s = string(part, if refusal { "refusal" } else { "text" })?;
                                    if kind == "reasoning" {
                                        reasoning.push_str(s);
                                    } else {
                                        text.push_str(s);
                                    }
                                }
                            }
                        }
                    }
                    _ => return Err(Error::Evidence),
                }
            }
        }
        _ => return Err(Error::Policy),
    }
    let units = metered_output_units(&text, &reasoning, &calls);
    Ok((
        units,
        if incomplete {
            Disposition::Incomplete
        } else if refused {
            Disposition::Refused
        } else {
            Disposition::Complete
        },
    ))
}

/// A verified cumulative offer subtotal. Still requires canonical authorization,
/// remaining exposure, receipt/fee and rail settlement checks in the parent.
#[derive(Debug, Serialize)]
pub struct VerifiedSubtotal {
    pub invocation: Digest,
    pub attempt: u64,
    pub binding: Binding,
    pub result_digest: Digest,
    pub observation: Observation,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub subtotal_au: MoneyAu,
}
impl VerifiedSubtotal {
    pub fn digest(&self) -> Result<Digest> {
        endpoint::digest(
            "mayhem/proxy/verified-subtotal/v1",
            &serde_json::to_value(self).map_err(|_| Error::Evidence)?,
        )
        .map_err(|_| Error::Evidence)
    }
}
/// Exact-key owned recovery is the evidence source, not an upstream usage total.
/// `remaining_au` must come from locked parent authorization, never the connector.
/// This performs no I/O, payout, hold release, receipt lookup or current-offer lookup.
pub fn price_completed(recovery: &Recovery, remaining_au: MoneyAu) -> Result<VerifiedSubtotal> {
    let r = &recovery.record;
    let b = &r.binding;
    let accepted = recovery.acceptance.as_ref().ok_or(Error::Offer)?;
    accepted.validate_for(r).map_err(|_| Error::Offer)?;
    let accepted_offer = &accepted.snapshot.offer;
    let input = &recovery.request.as_ref().ok_or(Error::Evidence)?.body;
    let output = recovery.result.as_ref().ok_or(Error::Evidence)?;
    let policy = Policy::resolve(b.endpoint, &b.metering_policy)?;
    let request: Value = serde_json::from_slice(input).map_err(|_| Error::Evidence)?;
    let prepared = policy.prepare(b.endpoint, &request)?;
    if prepared.request_hash != b.request_hash {
        return Err(Error::Evidence);
    }
    let observed = prepared.observe(&output.reply.body)?;
    if output.reply.observed_usage.as_ref() != Some(&observed) {
        return Err(Error::Evidence);
    }
    if observed.disposition != Disposition::Complete || r.cancellation_requested {
        return Err(Error::OutcomePolicyRequired);
    }
    if accepted_offer.digest().map_err(|_| Error::Offer)? != b.offer_digest.as_str()
        || accepted_offer.market_id != b.market_id.as_str()
        || accepted_offer.provider_pubkey != b.provider_pubkey.as_str()
        || accepted_offer.endpoint != b.endpoint
        || accepted_offer.metering_policy_hash != b.metering_policy.as_str()
        || !accepted_offer.accepted_rails.contains(&b.rail)
        || !accepted_offer
            .rates
            .iter()
            .map(|r| r.unit.as_str())
            .eq(policy.units().iter().copied())
    {
        return Err(Error::Offer);
    }
    let subtotal_au = accepted_offer
        .cost(&observed.units)
        .map_err(|_| Error::Overflow)?;
    if subtotal_au > remaining_au {
        return Err(Error::Budget);
    }
    Ok(VerifiedSubtotal {
        invocation: r.invocation.clone(),
        attempt: r.attempt,
        binding: b.clone(),
        result_digest: output.digest.clone(),
        observation: observed,
        subtotal_au,
    })
}

/// Prepare an UNSIGNED final receipt from owned, independently recounted output.
/// The parent supplies retained canonical terms/policy and an exact prior head.
/// This does not authorize signatures, debit a buyer, release a hold or update a
/// payout. Current offers, contract version and wall-clock quote expiry are not
/// consulted when recovering previously accepted work.
pub fn draft_completed_receipt(
    recovery: &Recovery,
    terms: &mayhem_proto::proxy::finance::ProxySpendTerms,
    policy: &mayhem_proto::proxy::finance::ProxySettlementPolicy,
    seq: u64,
    at_ms: u64,
    previous: Option<&mayhem_proto::proxy::finance::ProxyReceiptBody>,
) -> Result<mayhem_proto::proxy::finance::ProxyReceiptBody> {
    use mayhem_proto::proxy::finance::{ProxyReceiptBody, ProxyReceiptOutcome};
    let b = &recovery.record.binding;
    if terms.digest().map_err(|_| Error::Offer)? != b.accepted_terms.as_str()
        || terms.request_hash != b.request_hash.as_str()
        || terms.contract_version != b.contract_version
        || terms.reservation_id != b.reservation.as_str()
        || terms.capacity_lease != b.capacity_lease.as_str()
        || terms.endpoint_contract != b.endpoint_contract.as_str()
        || terms.recipe_hash != b.recipe_digest.as_str()
        || terms.connection_digest != b.connection_digest.as_str()
        || terms.connection_revision != b.connection_revision
        || terms.offer.digest().map_err(|_| Error::Offer)? != b.offer_digest.as_str()
        || terms.rail != b.rail
    {
        return Err(Error::Offer);
    }
    let verified = price_completed(recovery, terms.max_spend_au)?;
    let receipt = ProxyReceiptBody {
        schema_version: 1,
        lane: mayhem_proto::proxy::ProxyLane::Proxy,
        accepted_terms: b.accepted_terms.as_str().into(),
        seq,
        final_receipt: true,
        outcome: ProxyReceiptOutcome::Complete,
        result_hash: verified.result_digest.as_str().into(),
        observation_hash: verified.digest()?.as_str().into(),
        usage: verified.observation.units,
        au_owed_cum: verified.subtotal_au,
        billing_au_owed_cum: terms
            .prior_spend_au
            .checked_add(verified.subtotal_au)
            .ok_or(Error::Overflow)?,
        at_ms,
    };
    receipt
        .validate_for(terms, policy, previous)
        .map_err(|_| Error::Evidence)?;
    Ok(receipt)
}
