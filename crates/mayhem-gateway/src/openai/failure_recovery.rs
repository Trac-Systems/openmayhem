use super::*;
use mayhem_proto::{
    canonical_usage_receipt_hash, reservation_binding_matches, usage_reservation_close_feature,
    usage_reservation_close_signing_bytes, usage_reservation_close_value,
};

pub(super) fn persist_reservation(
    invocation: &GatewaySessionInvocation,
) -> Result<(), GatewaySessionError> {
    let Some(job) = invocation.job.as_ref() else {
        return Ok(());
    };
    let mut store = job.store.lock_recover("gateway job vault");
    let mut previous = store
        .active_recovery(&job.id)
        .and_then(|state| state.receipt.clone());
    if previous.as_ref().is_some_and(|raw| {
        raw.pointer("/body/reservation_id")
            .or_else(|| raw.pointer("/reservation/reservation_id"))
            == Some(&json!(invocation.spend_voucher.body.reservation_id))
    }) {
        return Ok(());
    }
    if let Some(Value::Object(raw)) = &mut previous {
        raw.remove("prior_attempt_receipt");
    }
    store.update_active_receipt(&job.id, json!({
        "prior_attempt_receipt": previous,
        "reservation": invocation.spend_voucher,
        "reconciliation": {
            "terminal_status": "failed", "terminal_error": "provider session ended without terminal accounting",
            "transport_peer": invocation.direct_peer()?,
        },
    }), now_secs()).map_err(GatewaySessionError::new)
}

fn binding(job: &StoredGatewayJob) -> Result<Value, GatewaySessionError> {
    let raw = job
        .receipt
        .as_ref()
        .ok_or_else(|| GatewaySessionError::new("missing reservation evidence"))?;
    if raw.get("canonical_settlement").is_some() && raw["body"]["model_id"] == job.model {
        // A crash may occur between storing closure proof and marking the job
        // terminal. Verify the canonical proof again below without pretending
        // its partial receipt is an ordinary final receipt.
        return Ok(raw["body"].clone());
    }
    if raw.get("body").is_some() {
        let recovery = parse_gateway_job_receipt_recovery(job)?;
        return serde_json::to_value(recovery.body)
            .map_err(|e| GatewaySessionError::new(e.to_string()));
    }
    let voucher: SpendVoucher = serde_json::from_value(raw["reservation"].clone())
        .map_err(|e| GatewaySessionError::new(format!("invalid recovery voucher: {e}")))?;
    if voucher.body.model_id != job.model {
        return Err(GatewaySessionError::new("recovery voucher model mismatch"));
    }
    let key: [u8; 32] = hex::decode(&voucher.body.user)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| GatewaySessionError::new("invalid recovery voucher user"))?;
    let signature: [u8; 64] = hex::decode(&voucher.user_sig)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| GatewaySessionError::new("invalid recovery voucher signature"))?;
    VerifyingKey::from_bytes(&key)
        .map_err(|e| GatewaySessionError::new(e.to_string()))?
        .verify_strict(
            &spend_voucher_signing_bytes(&voucher.body)
                .map_err(|e| GatewaySessionError::new(e.to_string()))?,
            &Signature::from_bytes(&signature),
        )
        .map_err(|e| GatewaySessionError::new(e.to_string()))?;
    serde_json::to_value(voucher.body).map_err(|e| GatewaySessionError::new(e.to_string()))
}

#[cfg(test)]
pub(super) fn test_closed_proof(receipt: &ProviderSignedReceipt, ack: &ReceiptAck) -> Value {
    let body = json!(receipt.body);
    let id = &receipt.body.reservation_id;
    let mut reservation = serde_json::Map::new();
    for key in mayhem_proto::RESERVATION_BINDING_FIELDS {
        reservation.insert((*key).into(), body[*key].clone());
    }
    let mut close = reservation.clone();
    reservation.insert("type".into(), json!("receipt_reservation_identity"));
    reservation.insert("status".into(), json!("closed"));
    reservation.insert(
        "close_record_key".into(),
        json!(format!("receipt/reservation-close/{id}")),
    );
    let envelope = json!({"body": receipt.body, "enclave_sig": receipt.enclave_sig,
        "enclave_pubkey": receipt.enclave_pubkey, "user_sig": ack.user_sig});
    let hash = canonical_usage_receipt_hash(&envelope).unwrap();
    let retained = (receipt.body.au_owed_cum - receipt.body.billing_prior_au_owed_cum).to_string();
    close.insert("type".into(), json!("targeted_reservation_close"));
    close.insert("latest_receipt_seq".into(), json!(receipt.body.seq));
    close.insert("latest_receipt_hash".into(), json!(hash));
    close.insert("retained_au".into(), json!(retained));
    json!({
        "reservation": {"key": format!("receipt/reservation/{id}"), "confirmed": true, "value": reservation},
        "close": {"key": format!("receipt/reservation-close/{id}"), "confirmed": true, "value": close},
        "head": {"key": format!("receipt/head/{}/{}", receipt.body.billing_id, receipt.body.billing_attempt),
            "confirmed": true, "value": {"type": "canonical_receipt_head", "settlement_ready": true,
                "receipt_seq": receipt.body.seq, "receipt_hash": hash, "receipt": envelope, "incremental_au": retained}},
    })
}

async fn confirmed(rpc: &PeerRpcClient, key: &str) -> Result<Value, GatewaySessionError> {
    let result = rpc
        .state(Some(key), Some(true))
        .await
        .map_err(|e| GatewaySessionError::retryable(e.to_string()))?;
    if result["confirmed"] != true || result["key"] != key {
        return Err(GatewaySessionError::new(
            "reservation recovery requires exact confirmed state",
        ));
    }
    Ok(result)
}

pub(super) fn verify_closed(
    binding: &Value,
    proof: &Value,
) -> Result<Option<SessionReceipt>, GatewaySessionError> {
    let fail = || {
        GatewaySessionError::new("canonical reservation closure proof does not match this request")
    };
    let id = binding["reservation_id"].as_str().ok_or_else(fail)?;
    let billing_id = binding["billing_id"].as_str().ok_or_else(fail)?;
    let attempt = binding["billing_attempt"].as_u64().ok_or_else(fail)?;
    for (field, key) in [
        ("reservation", format!("receipt/reservation/{id}")),
        ("close", format!("receipt/reservation-close/{id}")),
        ("head", format!("receipt/head/{billing_id}/{attempt}")),
    ] {
        if proof[field]["confirmed"] != true || proof[field]["key"] != key {
            return Err(fail());
        }
    }
    let reservation = &proof["reservation"]["value"];
    let close = &proof["close"]["value"];
    let head = &proof["head"]["value"];
    if reservation["type"] != "receipt_reservation_identity"
        || reservation["status"] != "closed"
        || close["type"] != "targeted_reservation_close"
        || reservation["close_record_key"] != format!("receipt/reservation-close/{id}")
        || !reservation_binding_matches(binding, reservation)
        || !reservation_binding_matches(binding, close)
    {
        return Err(fail());
    }
    if head.is_null() {
        if !close["latest_receipt_seq"].is_null()
            || !close["latest_receipt_hash"].is_null()
            || close["retained_au"] != "0"
        {
            return Err(fail());
        }
        return Ok(None);
    }
    let receipt =
        parse_record_usage_receipt_envelope(&head["receipt"]).map_err(GatewaySessionError::new)?;
    let body =
        serde_json::to_value(&receipt.body).map_err(|e| GatewaySessionError::new(e.to_string()))?;
    let hash = canonical_usage_receipt_hash(&head["receipt"]).map_err(GatewaySessionError::new)?;
    let retained = receipt
        .body
        .au_owed_cum
        .checked_sub(receipt.body.billing_prior_au_owed_cum)
        .ok_or_else(fail)?;
    if head["type"] != "canonical_receipt_head"
        || head["settlement_ready"] != true
        || !reservation_binding_matches(binding, &body)
        || body["model_id"] != binding["model_id"]
        || body["enclave_id"] != binding["enclave_id"]
        || head["receipt_seq"] != receipt.body.seq
        || head["receipt_hash"] != hash
        || close["latest_receipt_seq"] != receipt.body.seq
        || close["latest_receipt_hash"] != hash
        || close["retained_au"] != retained.to_string()
        || head["incremental_au"] != retained.to_string()
    {
        return Err(fail());
    }
    // Canonical state is authoritative only with the exact original signatures.
    verify_provider_receipt_signature(&ProviderSignedReceipt {
        body: receipt.body.clone(),
        enclave_sig: receipt.enclave_sig.clone(),
        enclave_pubkey: receipt.enclave_pubkey.clone(),
    })?;
    let key: [u8; 32] = hex::decode(&receipt.body.user)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(fail)?;
    let signature: [u8; 64] = hex::decode(&receipt.user_sig)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(fail)?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| fail())?
        .verify_strict(
            &receipt_signing_bytes(&receipt.body).map_err(|_| fail())?,
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| fail())?;
    Ok(Some(receipt))
}

pub(super) async fn reconcile(
    state: &GatewayState,
    job: &StoredGatewayJob,
) -> Result<(), GatewaySessionError> {
    let rpc = state
        .canary_probe_contract_rpc
        .as_ref()
        .as_ref()
        .ok_or_else(|| {
            GatewaySessionError::retryable("reservation recovery requires canonical ledger access")
        })?;
    let binding = binding(job)?;
    let id = binding["reservation_id"]
        .as_str()
        .ok_or_else(|| GatewaySessionError::new("missing reservation identity"))?;
    let reservation = confirmed(rpc, &format!("receipt/reservation/{id}")).await?;
    if !reservation_binding_matches(&binding, &reservation["value"]) {
        return Err(GatewaySessionError::retryable(
            "reservation identity is not confirmed yet",
        ));
    }
    let head = confirmed(
        rpc,
        &format!(
            "receipt/head/{}/{}",
            binding["billing_id"].as_str().unwrap_or_default(),
            binding["billing_attempt"].as_u64().unwrap_or(0)
        ),
    )
    .await?;
    if reservation["value"]["status"] != "closed" {
        // The provider can close immediately after ending compute. If it never
        // comes back, the buyer may close only after canonical expiry + grace.
        let epoch = confirmed(rpc, "epoch/apply/state").await?;
        let eligible = binding["reservation_expires_after_epoch"]
            .as_u64()
            .and_then(|n| n.checked_add(binding["reservation_receipt_grace_epochs"].as_u64()?));
        if !eligible
            .zip(epoch["value"]["updated_epoch"].as_u64())
            .is_some_and(|(limit, now)| now >= limit)
        {
            return Err(GatewaySessionError::retryable(
                "awaiting provider reservation closure or canonical expiry",
            ));
        }
        if binding["user"] != verifying_key_hex(&state.receipt_config.user_seed) {
            return Err(GatewaySessionError::new(
                "cannot expire another buyer's reservation",
            ));
        }
        let mut raw = job.receipt.clone().unwrap_or(Value::Null);
        let mut feature = raw["reconciliation"]["reservation_expiry_feature"].clone();
        if feature.is_null()
            || feature["value"]["latest_receipt_seq"] != head["value"]["receipt_seq"]
            || feature["value"]["latest_receipt_hash"] != head["value"]["receipt_hash"]
        {
            let mut value = usage_reservation_close_value(
                &binding,
                (!head["value"].is_null()).then_some(&head["value"]),
                true,
                now_secs(),
                "gateway_recovery",
            )
            .map_err(GatewaySessionError::new)?;
            value["actor_sig"] = json!(sign_hex(
                &state.receipt_config.user_seed,
                &usage_reservation_close_signing_bytes(&value).map_err(GatewaySessionError::new)?
            ));
            feature = usage_reservation_close_feature(value).map_err(GatewaySessionError::new)?;
            raw["reconciliation"]["reservation_expiry_feature"] = feature.clone();
            state
                .jobs
                .lock_recover("gateway job vault")
                .update_reconciliation_receipt(&job.id, raw, now_secs())
                .map_err(GatewaySessionError::new)?;
        }
        let response = rpc
            .submit_feature(feature)
            .await
            .map_err(|e| GatewaySessionError::retryable(e.to_string()))?;
        if response["ok"] != true {
            return Err(GatewaySessionError::retryable(
                "reservation expiry submission remains pending",
            ));
        }
        return Err(GatewaySessionError::retryable(
            "awaiting confirmed reservation expiry",
        ));
    }
    let close = confirmed(rpc, &format!("receipt/reservation-close/{id}")).await?;
    let proof = json!({"reservation": reservation, "head": head, "close": close});
    let receipt = verify_closed(&binding, &proof)?;
    let mut raw = job.receipt.clone().unwrap_or(Value::Null);
    raw["canonical_settlement"] = proof;
    if let Some(receipt) = receipt {
        // Retain final=false when the ledger closed a partial checkpoint.
        raw["body"] = json!(receipt.body);
        raw["enclave_sig"] = json!(receipt.enclave_sig);
        raw["enclave_pubkey"] = json!(receipt.enclave_pubkey);
        raw["receipt_ack"] = json!({"session_id": receipt.body.session_id,
            "seq": receipt.body.seq, "user_sig": receipt.user_sig});
    }
    let status = if job.error_info.is_some() {
        GatewayJobStatus::Failed
    } else {
        job.receipt
            .as_ref()
            .and_then(|v| v.pointer("/reconciliation/terminal_status"))
            .and_then(|v| serde_json::from_value::<GatewayJobStatus>(v.clone()).ok())
            .filter(|s| *s != GatewayJobStatus::ReconciliationPending)
            .unwrap_or(GatewayJobStatus::Failed)
    };
    let mut store = state.jobs.lock_recover("gateway job vault");
    store
        .update_reconciliation_receipt(&job.id, raw, now_secs())
        .map_err(GatewaySessionError::new)?;
    store
        .finish_reconciliation(&job.id, status, job.error.clone(), now_secs())
        .map_err(GatewaySessionError::new)?;
    Ok(())
}
