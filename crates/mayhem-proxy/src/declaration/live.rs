//! Descriptor metadata only. The verified revision floor and signed lifetime
//! survive source failures; only a fresh successful read grants a local lease.
use super::*;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

struct Observed {
    signed: Signed,
    digest: Digest,
    // A reread may shorten this deadline, never extend it for the same record.
    valid_until: Option<Instant>,
    expired: bool,
}
struct State {
    highest: Option<Observed>,
    read_at: Option<Instant>,
    reason: &'static str,
}
/// Bounded metadata status, not evidence that a provider obeys its promises.
#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub observed_revision: Option<u64>,
    pub observed_digest: Option<Digest>,
    pub state: &'static str,
}
pub(crate) struct Live {
    pub(crate) subject: Subject,
    current: Mutex<State>,
}
impl Live {
    pub(crate) fn new(subject: Subject) -> Self {
        Self {
            subject,
            current: Mutex::new(State {
                highest: None,
                read_at: None,
                reason: "missing",
            }),
        }
    }
    pub(crate) fn update(&self, signed: Option<Signed>, now: u64) -> Result<()> {
        self.update_at(signed, now, Instant::now())
    }
    fn update_at(&self, signed: Option<Signed>, now: u64, at: Instant) -> Result<()> {
        // Verification belongs to the bounded background update, never read().
        // Keep the failure until after acquiring state so invalid input also
        // hides any previously readable declaration.
        let checked = signed.map(|signed| {
            let digest = signed.digest()?;
            require(
                signed.body.subject == self.subject,
                "declaration source subject differs",
            )?;
            Ok::<_, crate::Error>((signed, digest))
        });
        let mut state = self
            .current
            .lock()
            .map_err(|_| crate::invalid("declaration state unavailable"))?;
        state.expire(Some(now), at);
        state.read_at = None;
        let (signed, digest) = match checked {
            None => {
                if !state.is_expired() {
                    state.reason = "missing";
                }
                return Ok(());
            }
            Some(Err(error)) => {
                if !state.is_expired() {
                    state.reason = "invalid";
                }
                return Err(error);
            }
            Some(Ok(value)) => value,
        };
        if let Some(prior) = &state.highest {
            if signed.body.revision < prior.signed.body.revision {
                if !state.is_expired() {
                    state.reason = "regression";
                }
                return Err(crate::invalid("declaration revision regressed"));
            }
            if signed.body.revision == prior.signed.body.revision && digest != prior.digest {
                if !state.is_expired() {
                    state.reason = "equivocation";
                }
                return Err(crate::invalid("declaration revision equivocated"));
            }
        }
        let candidate = at.checked_add(Duration::from_millis(
            signed.body.expires_at_ms.saturating_sub(now),
        ));
        match &mut state.highest {
            Some(prior) if prior.signed.body.revision == signed.body.revision => {
                // Retain both deadline and expiry latch across None, invalid,
                // stale-lease, regression and equivocation updates.
                prior.valid_until = match (prior.valid_until, candidate) {
                    (Some(old), Some(next)) => Some(old.min(next)),
                    (old, next) => old.or(next),
                };
            }
            _ => {
                state.highest = Some(Observed {
                    signed,
                    digest,
                    valid_until: candidate,
                    expired: false,
                });
            }
        }
        // A valid higher record advances the floor even if it cannot currently
        // be served (expired/not yet valid). No lower record can return afterward.
        if state.expire(Some(now), at) {
            return Err(crate::invalid("declaration expired"));
        }
        let current = state
            .highest
            .as_ref()
            .ok_or_else(|| crate::invalid("declaration state unavailable"))?;
        if current.valid_until.is_none() {
            state.reason = "clock_unavailable";
            return Err(crate::invalid(
                "declaration expiry outside local clock range",
            ));
        }
        if now < current.signed.body.issued_at_ms {
            state.reason = "not_yet_valid";
            return Err(crate::invalid("declaration not yet valid"));
        }
        state.read_at = Some(at);
        state.reason = "available";
        Ok(())
    }
    pub(crate) fn read(&self, now: u64) -> Option<Signed> {
        self.read_at(now, Instant::now())
    }
    fn read_at(&self, now: u64, at: Instant) -> Option<Signed> {
        let mut state = self.current.lock().ok()?;
        if !state.available(now, at) {
            return None;
        }
        // No signature checks, serialization, file reads or financial operations.
        Some(state.highest.as_ref()?.signed.clone())
    }
    /// The runner persists this exact latch before any subsequent activation.
    /// This value never authorizes fallback to checkpoint claims.
    pub(crate) fn expired_identity(&self) -> Option<(u64, Digest)> {
        self.expired_identity_at(Instant::now())
    }
    fn expired_identity_at(&self, at: Instant) -> Option<(u64, Digest)> {
        let mut state = self.current.lock().ok()?;
        state.expire(None, at);
        let highest = state.highest.as_ref().filter(|v| v.expired)?;
        Some((highest.signed.body.revision, highest.digest.clone()))
    }
    pub(crate) fn inspect(&self, now: u64) -> Inspection {
        self.inspect_at(now, Instant::now())
    }
    fn inspect_at(&self, now: u64, at: Instant) -> Inspection {
        let Ok(mut state) = self.current.lock() else {
            return Inspection {
                observed_revision: None,
                observed_digest: None,
                state: "state_unavailable",
            };
        };
        state.available(now, at);
        Inspection {
            observed_revision: state.highest.as_ref().map(|v| v.signed.body.revision),
            observed_digest: state.highest.as_ref().map(|v| v.digest.clone()),
            state: state.reason,
        }
    }
}
impl State {
    fn is_expired(&self) -> bool {
        self.highest.as_ref().is_some_and(|v| v.expired)
    }
    fn expire(&mut self, now: Option<u64>, at: Instant) -> bool {
        let Some(highest) = &mut self.highest else {
            return false;
        };
        if highest.expired
            || highest.valid_until.is_some_and(|until| at >= until)
            || now.is_some_and(|now| now >= highest.signed.body.expires_at_ms)
        {
            highest.expired = true;
            self.read_at = None;
            self.reason = "expired";
            true
        } else {
            false
        }
    }
    fn available(&mut self, now: u64, at: Instant) -> bool {
        if self.expire(Some(now), at) {
            return false;
        }
        let Some(highest) = &self.highest else {
            return false;
        };
        let Some(read_at) = self.read_at else {
            return false;
        };
        if at.saturating_duration_since(read_at) >= Duration::from_millis(MAX_READ_AGE_MS) {
            self.read_at = None;
            self.reason = "read_lease_expired";
            return false;
        }
        if highest.valid_until.is_none() {
            self.reason = "clock_unavailable";
            return false;
        }
        if now < highest.signed.body.issued_at_ms {
            self.reason = "not_yet_valid";
            return false;
        }
        self.reason = "available";
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn digest(n: u8) -> Digest {
        Digest::new(format!("{n:064x}")).unwrap()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[217; 32])
    }
    fn subject() -> Subject {
        Subject {
            network: discovery::Identity {
                network_id: "live-declaration-fixture".into(),
                msb_bootstrap: digest(1).as_str().into(),
                subnet_bootstrap: digest(2).as_str().into(),
                contract_version: mayhem_proto::CONTRACT_VERSION,
            },
            provider: Digest::new(hex(&key().verifying_key().to_bytes())).unwrap(),
            market: digest(3),
            membership_revision: 1,
            membership_digest: digest(4),
            endpoint: ProxyEndpoint::Chat,
            endpoint_contract: digest(5),
            recipe_hash: digest(6),
            connection_revision: 1,
        }
    }
    fn sign(body: Body) -> Signed {
        Signed {
            signature: hex(&key().sign(&body.signing_bytes().unwrap()).to_bytes()),
            body,
        }
    }
    fn record(revision: u64, issued_at_ms: u64, expires_at_ms: u64) -> Signed {
        sign(Body {
            schema_version: 1,
            subject: subject(),
            revision,
            issued_at_ms,
            expires_at_ms,
            claims: vec![Claim {
                field_id: "privacy.training".into(),
                schema_revision: 1,
                definition_digest: digest(7),
                status: registry::Support::Supported,
                value: Some(registry::TypedValue::Boolean(false)),
            }],
        })
    }
    fn later(base: Instant, milliseconds: u64) -> Instant {
        base + Duration::from_millis(milliseconds)
    }

    #[test]
    fn live_read_lease_expires_without_a_poller_and_exact_refresh_recovers() {
        let live = Live::new(subject());
        let base = Instant::now();
        let signed = record(1, 100, 100_000);
        live.update_at(Some(signed.clone()), 100, base).unwrap();
        assert!(live.read_at(15_099, later(base, 14_999)).is_some());
        assert!(live.read_at(15_100, later(base, 15_000)).is_none());
        assert_eq!(
            live.inspect_at(15_100, later(base, 15_000)).state,
            "read_lease_expired"
        );
        assert!(live.expired_identity_at(later(base, 15_000)).is_none());
        live.update_at(Some(signed), 15_101, later(base, 15_001))
            .unwrap();
        assert!(live.read_at(15_101, later(base, 15_001)).is_some());
    }

    #[test]
    fn live_refresh_and_failure_cannot_extend_original_monotonic_expiry() {
        let live = Live::new(subject());
        let base = Instant::now();
        let signed = record(1, 100, 1_100);
        let identity = (1, signed.digest().unwrap());
        live.update_at(Some(signed.clone()), 100, base).unwrap();
        live.update_at(None, 101, later(base, 400)).unwrap();
        assert!(live.read_at(101, later(base, 400)).is_none());
        // Wall clock advanced 1 ms while monotonic time advanced 800 ms.
        live.update_at(Some(signed.clone()), 101, later(base, 800))
            .unwrap();
        assert!(live.read_at(101, later(base, 999)).is_some());
        assert!(live.read_at(101, later(base, 1_000)).is_none());
        assert_eq!(live.expired_identity_at(later(base, 1_000)), Some(identity));
        assert!(live
            .update_at(Some(signed), 102, later(base, 1_001))
            .is_err());
        assert!(live.read_at(102, later(base, 1_001)).is_none());
    }

    #[test]
    fn live_wall_expiry_is_latched_across_rollback_and_missing_input() {
        let live = Live::new(subject());
        let base = Instant::now();
        let signed = record(1, 100, 1_100);
        live.update_at(Some(signed.clone()), 100, base).unwrap();
        assert!(live.read_at(1_100, later(base, 1)).is_none());
        live.update_at(None, 101, later(base, 2)).unwrap();
        assert!(live
            .update_at(Some(signed.clone()), 101, later(base, 3))
            .is_err());
        assert_eq!(
            live.expired_identity_at(later(base, 3)),
            Some((1, signed.digest().unwrap()))
        );
    }

    #[test]
    fn live_invalid_and_wrong_subject_updates_hide_without_poisoning_the_floor() {
        let live = Live::new(subject());
        let base = Instant::now();
        let original = record(1, 100, 100_000);
        live.update_at(Some(original.clone()), 100, base).unwrap();
        let mut invalid = record(9, 100, 100_000);
        invalid.signature = "00".repeat(64);
        assert!(live.update_at(Some(invalid), 101, later(base, 1)).is_err());
        assert!(live.read_at(101, later(base, 1)).is_none());
        assert_eq!(
            live.inspect_at(101, later(base, 1)).observed_revision,
            Some(1)
        );
        live.update_at(Some(original.clone()), 102, later(base, 2))
            .unwrap();
        let mut wrong = record(10, 100, 100_000).body;
        wrong.subject.connection_revision = 2;
        assert!(live
            .update_at(Some(sign(wrong)), 103, later(base, 3))
            .is_err());
        assert!(live.read_at(103, later(base, 3)).is_none());
        assert_eq!(
            live.inspect_at(103, later(base, 3)).observed_revision,
            Some(1)
        );
        live.update_at(Some(original), 104, later(base, 4)).unwrap();
        assert!(live.read_at(104, later(base, 4)).is_some());
    }

    #[test]
    fn live_regression_and_equivocation_hide_but_exact_latest_can_recover() {
        let live = Live::new(subject());
        let base = Instant::now();
        let original = record(2, 100, 100_000);
        let expected = original.digest().unwrap();
        live.update_at(Some(original.clone()), 100, base).unwrap();
        assert!(live
            .update_at(Some(record(1, 100, 100_000)), 101, later(base, 1))
            .is_err());
        assert!(live.read_at(101, later(base, 1)).is_none());
        assert_eq!(live.inspect_at(101, later(base, 1)).state, "regression");
        let mut equivocation = original.body.clone();
        equivocation.claims[0].value = Some(registry::TypedValue::Boolean(true));
        assert!(live
            .update_at(Some(sign(equivocation)), 102, later(base, 2))
            .is_err());
        let status = live.inspect_at(102, later(base, 2));
        assert_eq!(status.state, "equivocation");
        assert_eq!(status.observed_digest, Some(expected));
        assert!(live.read_at(102, later(base, 2)).is_none());
        live.update_at(Some(original), 103, later(base, 3)).unwrap();
        assert!(live.read_at(103, later(base, 3)).is_some());
    }

    #[test]
    fn live_higher_expired_revision_blocks_older_claims_until_explicit_renewal() {
        let live = Live::new(subject());
        let base = Instant::now();
        let original = record(1, 100, 100_000);
        live.update_at(Some(original.clone()), 100, base).unwrap();
        let expired = record(2, 100, 101);
        let expired_digest = expired.digest().unwrap();
        assert!(live.update_at(Some(expired), 102, later(base, 2)).is_err());
        assert_eq!(
            live.expired_identity_at(later(base, 2)),
            Some((2, expired_digest))
        );
        assert!(live.update_at(Some(original), 103, later(base, 3)).is_err());
        live.update_at(Some(record(3, 104, 200_000)), 104, later(base, 4))
            .unwrap();
        assert_eq!(live.read_at(104, later(base, 4)).unwrap().body.revision, 3);
        assert!(live.expired_identity_at(later(base, 4)).is_none());
    }

    #[test]
    fn live_future_signed_revision_advances_floor_without_becoming_available() {
        let live = Live::new(subject());
        let base = Instant::now();
        let signed = record(2, 200, 1_000);
        assert!(live.update_at(Some(signed.clone()), 100, base).is_err());
        assert_eq!(live.inspect_at(100, base).state, "not_yet_valid");
        assert!(live
            .update_at(Some(record(1, 100, 1_000)), 101, later(base, 1))
            .is_err());
        assert!(live.read_at(200, later(base, 100)).is_none());
        live.update_at(Some(signed), 200, later(base, 100)).unwrap();
        assert!(live.read_at(200, later(base, 100)).is_some());
        // Even a fresh lease cannot expose it before signed issuance after rollback.
        assert!(live.read_at(199, later(base, 101)).is_none());
        assert!(live.read_at(201, later(base, 102)).is_some());
    }

    #[test]
    fn live_unknown_withdrawal_replaces_claims_and_inspection_exposes_only_identity() {
        let live = Live::new(subject());
        let base = Instant::now();
        live.update_at(Some(record(1, 100, 100_000)), 100, base)
            .unwrap();
        let mut withdrawn = record(2, 101, 100_000).body;
        withdrawn.claims[0].status = registry::Support::Unknown;
        withdrawn.claims[0].value = None;
        live.update_at(Some(sign(withdrawn)), 101, later(base, 1))
            .unwrap();
        let current = live.read_at(101, later(base, 1)).unwrap();
        assert!(matches!(
            current.body.claims[0].status,
            registry::Support::Unknown
        ));
        assert!(current.body.claims[0].value.is_none());
        let view = serde_json::to_value(live.inspect_at(101, later(base, 1))).unwrap();
        assert_eq!(view.as_object().unwrap().len(), 3);
        assert_eq!(view["state"], "available");
        assert_eq!(view["observed_revision"], 2);
        assert!(view.get("claims").is_none());
        assert!(view.get("signature").is_none());
    }
}
