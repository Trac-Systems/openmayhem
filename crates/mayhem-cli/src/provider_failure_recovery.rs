//! Durable reservation recovery. A held OS lock means generation still owns
//! the reservation; returning, panicking, or process exit releases that lock.
use super::*;
use mayhem_proto::{
    reservation_binding_matches, usage_reservation_close_feature,
    usage_reservation_close_signing_bytes, usage_reservation_close_value,
};

#[derive(Debug)]
pub(super) struct AttemptGuard {
    _lock: fs::File,
    path: PathBuf,
}

fn directory(settlement: &ProviderReceiptSettlement) -> PathBuf {
    settlement.outbox.directory.join("reservation-recovery")
}

fn journal_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new("json")) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn lock(path: &Path) -> Result<fs::File> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "invalid reservation lock file"
        );
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub(super) fn begin(
    active: &ActiveProviderSession,
    terms: &ProviderSessionTerms,
) -> Result<Option<AttemptGuard>> {
    let Some(settlement) = active.receipt_settlement.as_ref() else {
        return Ok(None);
    };
    let binding = json!({
        "billing_epoch": active.billing_epoch, "reservation_id": active.reservation_id,
        "reservation_expires_after_epoch": active.reservation_expires_after_epoch,
        "reservation_receipt_grace_epochs": active.reservation_receipt_grace_epochs,
        "billing_id": active.billing_id, "billing_attempt": active.billing_attempt,
        "session_id": active.session_id, "user": active.user_pubkey, "rail": active.rail,
        "provider": terms.provider, "payout_revision": active.payout_revision,
        "model_id": terms.model_id, "enclave_id": terms.enclave_id,
    });
    begin_binding(settlement, &binding).map(Some)
}

pub(super) fn begin_binding(
    settlement: &ProviderReceiptSettlement,
    binding: &Value,
) -> Result<AttemptGuard> {
    let root = directory(settlement);
    ensure_private_directory(&root, "provider reservation recovery")?;
    let reservation_id = binding["reservation_id"]
        .as_str()
        .context("reservation recovery binding has no identity")?;
    ensure!(
        is_hex_len(reservation_id, 64) && reservation_binding_matches(binding, binding),
        "invalid reservation identity"
    );
    ensure!(
        binding["model_id"]
            .as_str()
            .is_some_and(|model| !model.is_empty())
            && binding["enclave_id"]
                .as_str()
                .is_some_and(|enclave| is_hex_len(enclave, 64)),
        "reservation recovery binding is missing model evidence"
    );
    let path = root.join(format!("{reservation_id}.json"));
    ensure!(
        path.exists() || journal_paths(&root)?.len() < RECEIPT_SETTLEMENT_OUTBOX_MAX_ENTRIES,
        "provider reservation recovery is full; refusing new compute until recovery progresses"
    );
    let held = lock(&path.with_extension("lock"))?;
    fs2::FileExt::try_lock_exclusive(&held)
        .context("reservation is already executing or recovering")?;
    // No prompts, credential material, or card/account funding data.
    write_private_json_once(&path, &json!({"schema_version": 1, "binding": binding}))?;
    Ok(AttemptGuard { _lock: held, path })
}

pub(super) fn discard(guard: AttemptGuard) -> Result<()> {
    let path = guard.path.clone();
    if path.exists() {
        fs::remove_file(&path)?;
        sync_receipt_settlement_directory(path.parent().context("recovery parent missing")?)?;
    }
    let lock_path = path.with_extension("lock");
    drop(guard);
    match fs::remove_file(lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn confirmed(rpc: &PeerRpcClient, key: &str) -> Result<Value> {
    let record = rpc.state(Some(key), Some(true)).await?;
    ensure!(
        record["confirmed"] == true && record["key"] == key,
        "reservation recovery needs exact confirmed state"
    );
    Ok(record["value"].clone())
}

pub(super) async fn recover_one(
    settlement: &ProviderReceiptSettlement,
    rpc: &PeerRpcClient,
    provider: &str,
    path: &Path,
) -> Result<bool> {
    let held = lock(&path.with_extension("lock"))?;
    if fs2::FileExt::try_lock_exclusive(&held).is_err() {
        return Ok(false);
    }
    let Some(entry) = read_private_json_optional(path)? else {
        return Ok(false);
    };
    ensure!(
        entry["schema_version"] == 1,
        "unsupported reservation recovery journal"
    );
    let binding = &entry["binding"];
    if binding["provider"].as_str() != Some(provider) {
        return Ok(false);
    }
    let id = binding["reservation_id"]
        .as_str()
        .filter(|id| is_hex_len(id, 64))
        .context("reservation recovery has invalid identity")?;
    ensure!(
        path.file_stem().and_then(OsStr::to_str) == Some(id),
        "reservation journal filename mismatch"
    );
    let reservation = confirmed(rpc, &format!("receipt/reservation/{id}")).await?;
    if reservation.is_null() {
        // The provider can observe the confirmed envelope before this key is
        // visible in its local canonical view. That is pending propagation,
        // not evidence that a different reservation owns this identity.
        let state = confirmed(rpc, "epoch/apply/state").await?;
        let updated_epoch = state["updated_epoch"]
            .as_u64()
            .context("reservation recovery epoch state is invalid")?;
        let abandon_after = binding["reservation_expires_after_epoch"]
            .as_u64()
            .context("reservation recovery expiry is invalid")?
            .saturating_add(
                binding["reservation_receipt_grace_epochs"]
                    .as_u64()
                    .context("reservation recovery grace is invalid")?,
            );
        if updated_epoch > abandon_after {
            fs::remove_file(path)?;
            sync_receipt_settlement_directory(path.parent().context("recovery parent missing")?)?;
            return Ok(true);
        }
        return Ok(false);
    }
    ensure!(
        reservation["type"] == "receipt_reservation_identity"
            && reservation_binding_matches(binding, &reservation),
        "reservation recovery binding mismatch"
    );
    if reservation["status"] == "closed" {
        let close = confirmed(rpc, &format!("receipt/reservation-close/{id}")).await?;
        ensure!(
            close["type"] == "targeted_reservation_close"
                && reservation_binding_matches(binding, &close),
            "reservation closure binding mismatch"
        );
        fs::remove_file(path)?;
        sync_receipt_settlement_directory(path.parent().context("recovery parent missing")?)?;
        return Ok(true);
    }
    ensure!(
        reservation["status"] == "active",
        "unknown reservation state"
    );
    // Give already-signed final or checkpoint receipts their durable delivery
    // opportunity before freezing the ledger's high-water mark.
    if settlement.outbox.load_entries()?.iter().any(|entry| {
        entry.feature.pointer("/value/receipt/body/reservation_id")
            == Some(&binding["reservation_id"])
    }) {
        return Ok(false);
    }
    let head = confirmed(
        rpc,
        &format!(
            "receipt/head/{}/{}",
            binding["billing_id"]
                .as_str()
                .context("missing billing id")?,
            binding["billing_attempt"]
                .as_u64()
                .context("missing billing attempt")?
        ),
    )
    .await?;
    if !head.is_null() {
        ensure!(
            head["type"] == "canonical_receipt_head"
                && reservation_binding_matches(binding, &head["receipt"]["body"])
                && head["receipt"]["body"]["model_id"] == binding["model_id"]
                && head["receipt"]["body"]["enclave_id"] == binding["enclave_id"],
            "canonical usage does not match the ended provider attempt"
        );
        if head["settlement_ready"] == true {
            return Ok(false);
        }
    }
    let mut value = usage_reservation_close_value(
        binding,
        (!head.is_null()).then_some(&head),
        false,
        unix_epoch_seconds()?,
        "provider_session_ended",
    )
    .map_err(anyhow::Error::msg)?;
    // Persist the exact signed submission before sending. A lost HTTP reply
    // cannot create another economic operation on the next pass.
    let submission = path.with_extension("close");
    let mut pending = read_private_json_optional(&submission)?;
    if let Some(existing) = &pending {
        ensure!(
            reservation_binding_matches(binding, &existing["value"]),
            "pending close binding mismatch"
        );
        if existing["value"]["latest_receipt_seq"] != value["latest_receipt_seq"]
            || existing["value"]["latest_receipt_hash"] != value["latest_receipt_hash"]
        {
            // A delayed checkpoint can win the ledger race. The older close
            // can no longer apply to this confirmed head; rebuild durably.
            fs::remove_file(&submission)?;
            sync_receipt_settlement_directory(path.parent().context("recovery parent missing")?)?;
            pending = None;
        }
    }
    let feature = if let Some(existing) = pending {
        existing
    } else {
        let payload = usage_reservation_close_signing_bytes(&value).map_err(anyhow::Error::msg)?;
        value["actor_sig"] = json!(
            sign_hex(
                &settlement.keypair_path,
                &settlement.password,
                &hex_encode(&payload)
            )
            .await?
        );
        let feature = usage_reservation_close_feature(value).map_err(anyhow::Error::msg)?;
        write_private_json_once(&submission, &feature)?;
        feature
    };
    let response = rpc.submit_feature(feature).await?;
    ensure!(response["ok"] == true, "reservation close remains pending");
    // Confirmation is checked again on the next pass, including after restart.
    Ok(false)
}

pub(super) fn spawn(
    settlement: Arc<ProviderReceiptSettlement>,
    rpc: PeerRpcClient,
    provider: String,
) {
    tokio::spawn(async move {
        let mut after = String::new();
        loop {
            let root = directory(&settlement);
            let mut paths = match journal_paths(&root) {
                Ok(paths) => paths,
                Err(error) => {
                    eprintln!("Provider reservation recovery scan failed: {error:#}");
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            paths.sort();
            let pivot =
                paths.partition_point(|path| path.to_string_lossy().as_ref() <= after.as_str());
            paths.rotate_left(pivot);
            for path in paths.into_iter().take(8) {
                after = path.to_string_lossy().into_owned();
                match timeout(
                    Duration::from_secs(20),
                    recover_one(&settlement, &rpc, &provider, &path),
                )
                .await
                {
                    Ok(Ok(true)) => {
                        let _ = fs::remove_file(path.with_extension("close"));
                        let _ = fs::remove_file(path.with_extension("lock"));
                    }
                    Ok(Ok(false)) => {}
                    Ok(Err(error)) => {
                        eprintln!("Provider reservation recovery remains pending: {error:#}")
                    }
                    Err(_) => eprintln!(
                        "Provider reservation recovery timed out; durable evidence retained"
                    ),
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    });
}
