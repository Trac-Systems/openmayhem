//! Final confirmation belongs to the exact signed outcome. The provider's
//! transport reply is useful progress, but cannot override canonical settlement.
use super::*;
use tokio::time::Instant;

impl Shared {
    /// Terminal operation: the caller returns immediately and drops both channel
    /// halves. A receive interrupted by canonical completion is never reused.
    pub(super) async fn acknowledge(
        &self,
        sender: &mut exchange::Sender,
        receiver: &mut exchange::Receiver,
        authorization: &ProxySpendAuthorization,
        key: Digest,
        p: &mut Progress,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<FinancialOutcome> {
        p.stage = Stage::Acknowledging;
        let acknowledgment = self
            .recovery
            .sign_approved(&self.signer, key)
            .await
            .map_err(|_| p.error(Code::Storage))?;
        p.check(stop)?;
        let deadline = Instant::now() + self.admission_wait;
        let unknown = AtomicBool::new(false);
        let wire_progress = tokio::sync::Notify::new();
        // One send only, using the durably retained signature. A lost bridge ACK
        // may still have delivered it; the provider's publication can outlive the
        // transport. Neither this future nor reconciliation dispatches inference.
        let wire = async {
            sender
                .send(&Message::Acknowledge {
                    acknowledgment: acknowledgment.clone(),
                })
                .await
                .map_err(|_| ())?;
            let response = receiver
                .receive(Some(self.limits.control_wait))
                .await
                .map_err(|_| ())?;
            Ok::<_, ()>(matches!(
                response.message(),
                Message::State {
                    state: PublicState::Settled | PublicState::AwaitingReceipt
                }
            ))
        };
        let confirmation = async {
            loop {
                match self
                    .recovery
                    .refresh(authorization, crate::supervisor::unix_ms())
                    .await
                {
                    Ok(Some(settlement)) => {
                        let exact = match (&acknowledgment, &settlement) {
                            (
                                SignedAcknowledgment::Receipt { receipt: expected },
                                FinancialOutcome::Paid { receipt },
                            ) => expected == receipt,
                            (
                                SignedAcknowledgment::Waiver { closure: expected },
                                FinancialOutcome::Waived { closure },
                            ) => expected == closure,
                            _ => false,
                        };
                        if !exact {
                            return Err(p.error(Code::Verification));
                        }
                        return Ok(settlement);
                    }
                    Ok(None) => (),
                    Err(error) if financial::observation_retryable(&error) => {
                        if financial::diagnostic_code(&error) == "proxy_financial_unknown" {
                            unknown.store(true, Ordering::Relaxed);
                        }
                    }
                    Err(error) => return Err(p.financial_error(error)),
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => (),
                    // One retained notification avoids adding the polling
                    // interval to a healthy Settled reply. Never cancels a read.
                    _ = wire_progress.notified() => (),
                }
            }
        };
        tokio::pin!(wire, confirmation);
        let mut wire_finished = false;
        loop {
            tokio::select! {
                biased;
                _ = stopped(stop) => return Err(p.error(Code::Stopped)),
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(p.financial_error(financial::admission_timeout(Some(&unknown))));
                },
                result = &mut confirmation => return result,
                result = &mut wire, if !wire_finished => {
                    wire_finished = true;
                    if matches!(result, Ok(false)) {
                        return Err(p.error(Code::Verification));
                    }
                    wire_progress.notify_one();
                    // Settled, AwaitingReceipt, or lost transport all still
                    // require the same fresh, exact canonical outcome above.
                },
            }
        }
    }
}
