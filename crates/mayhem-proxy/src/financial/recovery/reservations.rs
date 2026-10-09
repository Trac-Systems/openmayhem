//! Durable publication of an already dual-signed spend authorization. This does
//! not negotiate terms, sign arbitrary requests, allocate model capacity or send
//! inference. A missing acknowledgment never creates a new reservation identity.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReservationIntent {
    pub(super) at: u64,
    pub(super) proof: Option<Proof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) unadmitted: Option<intent::RetainedAbsence>,
}
impl ReservationIntent {
    pub(super) fn validate(&self) -> Result<()> {
        require(
            self.at <= PROXY_MAX_SAFE_INTEGER,
            "invalid reservation publication time",
        )?;
        if let Some(proof) = &self.proof {
            proof.validate()?;
        }
        Ok(())
    }
    pub(super) fn status(&self) -> ReservationStatus {
        ReservationStatus {
            at: self.at,
            confirmed: self.proof.is_some(),
            expired_unadmitted: self.unadmitted.is_some(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReservationStatus {
    pub at: u64,
    /// Historical confirmation only, never fresh dispatch eligibility.
    pub confirmed: bool,
    /// Canonically never admitted; no buyer hold or backend execution is claimed.
    pub expired_unadmitted: bool,
}

impl Store {
    fn retain_reservation(
        &self,
        authorization: ProxySpendAuthorization,
        policy: ProxySettlementPolicy,
        at: u64,
    ) -> Result<Digest> {
        let r = Record {
            authorization,
            policy,
            draft: None,
            signed: None,
            confirmed: None,
            proof: None,
            confirmed_epoch: None,
            closed_at: None,
            prune_after: None,
            acknowledgment: None,
            reservation: Some(ReservationIntent {
                at,
                proof: None,
                unadmitted: None,
            }),
        };
        let key = r.key()?;
        r.validate(&key, &self.identity)?;
        let tx = self.transaction()?;
        let mut records = crate::db(tx.open_table(RECORDS))?;
        if let Some(saved) = crate::db(records.get(key.as_str()))? {
            let old = self.decode(&key, saved.value())?;
            require(
                old.authorization == r.authorization && old.policy == r.policy,
                "saved reservation authorization changed",
            )?;
            // Preserve the first envelope and proof, including across upgrades.
            return Digest::new(key).map_err(|_| invalid("invalid terms key"));
        }
        require(
            r.authorization.terms.contract_version == mayhem_proto::CONTRACT_VERSION,
            "new reservation needs the current contract version",
        )?;
        let mut meta = crate::db(tx.open_table(META))?;
        let mut m = decode_meta(
            crate::db(meta.get("state"))?
                .ok_or_else(|| invalid("missing recovery metadata"))?
                .value(),
        )?;
        require(
            m.records < self.limits.max_records,
            "buyer recovery store is full",
        )?;
        self.save(&mut records, &key, &r)?;
        m.records += 1;
        crate::db(meta.insert("state", serde_json::to_vec(&m)?.as_slice()))?;
        crate::db(crate::db(tx.open_table(PENDING))?.insert(key.as_str(), 0))?;
        drop(meta);
        drop(records);
        self.commit(tx)?;
        Digest::new(key).map_err(|_| invalid("invalid terms key"))
    }
    fn close_unadmitted_reservation(
        &self,
        key: &str,
        o: intent::Observation,
        at_ms: u64,
    ) -> Result<()> {
        let tx = self.transaction()?;
        let mut table = crate::db(tx.open_table(RECORDS))?;
        let mut r = self.decode(
            key,
            crate::db(table.get(key))?
                .ok_or_else(|| invalid("unknown reservation intention"))?
                .value(),
        )?;
        let offer = negotiation::BuyerOffer {
            terms: r.authorization.terms.clone(),
            buyer_sig: r.authorization.buyer_sig.clone(),
        };
        let absent = o.absent(&offer, self.identity.controller_pubkey.as_str())?;
        let original = r
            .reservation
            .as_mut()
            .ok_or_else(|| invalid("not a reservation intention"))?;
        require(
            original.proof.is_none() && r.proof.is_none() && r.confirmed.is_none(),
            "admitted reservation cannot expire as unadmitted",
        )?;
        if original.unadmitted.is_some() {
            return Ok(());
        }
        require(
            at_ms >= original.at && at_ms <= PROXY_MAX_SAFE_INTEGER,
            "invalid reservation closure time",
        )?;
        let deadline = at_ms
            .checked_add(self.limits.closed_retention_ms)
            .filter(|n| *n <= PROXY_MAX_SAFE_INTEGER)
            .ok_or_else(|| invalid("recovery retention overflow"))?;
        original.unadmitted = Some(absent);
        r.closed_at = Some(at_ms);
        r.prune_after = Some(deadline);
        self.save(&mut table, key, &r)?;
        crate::db(crate::db(tx.open_table(PENDING))?.remove(key))?;
        crate::db(
            crate::db(tx.open_table(CLOSED))?.insert(format!("{deadline:020}/{key}").as_str(), key),
        )?;
        drop(table);
        o.fresh()?;
        self.commit(tx)
    }
}
impl BuyerRecovery {
    /// Persist exact buyer/provider signatures, original policy and publication
    /// timestamp before any append. Does not yet reserve money or execute work.
    pub async fn retain_reservation(
        &self,
        authorization: ProxySpendAuthorization,
        policy: ProxySettlementPolicy,
        at: u64,
    ) -> Result<Digest> {
        self.run(move |s| s.retain_reservation(authorization, policy, at))
            .await
    }

    /// Replay the same envelope through the existing durable canonical publication
    /// journal, then require a fresh signed canonical observation. Pending/lost
    /// publication remains recoverable; callers must not start a replacement hold.
    pub async fn publish_reservation(&self, key: Digest, at_ms: u64) -> Result<Observation> {
        let r = self.run(move |s| s.get(key.as_str())).await?;
        require(
            !r.expired_unadmitted(),
            "reservation intention expired without admission",
        )?;
        let submission = match r.reservation.as_ref().filter(|v| v.proof.is_none()) {
            Some(intent) => {
                self.client
                    .submit_reservation(&r.authorization, intent.at)
                    .await
            }
            None => Ok(()), // Already observed, or a pre-existing recovery record.
        };
        let observed = match self.client.observe(&r.authorization).await {
            Ok(value) => value,
            Err(error) => {
                // A missing observation alone proves nothing. Only a fresh
                // canonical non-admission proof can retire this publication.
                if r.reservation.as_ref().is_some_and(|v| v.proof.is_none()) {
                    let offer = negotiation::BuyerOffer {
                        terms: r.authorization.terms.clone(),
                        buyer_sig: r.authorization.buyer_sig.clone(),
                    };
                    if let Ok(o) = self.client.intent_state(&offer).await {
                        if o.status()? == intent::Status::Expired {
                            let key = r.key()?;
                            self.run(move |s| s.close_unadmitted_reservation(&key, o, at_ms))
                                .await?;
                            return Err(invalid("reservation intention expired without admission"));
                        }
                    }
                }
                submission?;
                return Err(error);
            }
        };
        self.run(move |s| {
            s.observe(&observed, false, at_ms)?;
            Ok(observed)
        })
        .await
    }
}
