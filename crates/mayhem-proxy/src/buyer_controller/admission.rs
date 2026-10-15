//! Bounded initial funding on the original accepted channel. A Status roundtrip
//! never dispatches inference or renews financial evidence.
use super::*;
use tokio::time::{timeout_at, Instant};

impl Shared {
    pub(super) async fn confirm_foreground(
        &self,
        paid: &mut exchange::Channel,
        key: Digest,
        p: &Progress,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<(SavedPurchase, financial::Observation)> {
        let deadline = Instant::now() + self.admission_wait;
        let unknown = AtomicBool::new(false);
        let confirmation = self.negotiation.publish_foreground(
            key,
            &self.recovery,
            crate::supervisor::unix_ms(),
            deadline,
            &unknown,
        );
        tokio::pin!(confirmation);
        let heartbeat = Duration::from_secs(1)
            .min(self.limits.control_wait / 2)
            .max(Duration::from_nanos(1));
        let mut next_status = Instant::now() + heartbeat;
        loop {
            tokio::select! {
                biased;
                _ = stopped(stop) => return Err(p.error(Code::Stopped)),
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(p.financial_error(financial::admission_timeout(Some(&unknown))));
                },
                result = &mut confirmation => return result.map_err(|error| p.financial_error(error)),
                _ = tokio::time::sleep_until(next_status) => {
                    let control_deadline = deadline.min(Instant::now() + self.limits.control_wait);
                    // Complete this exact roundtrip before polling confirmation
                    // again. Dropping a partially read frame would poison the
                    // channel; a late Prepared must not leak into result handling.
                    let exchange = async {
                        paid.send(&Message::Status).await?;
                        paid.receive(None).await
                    };
                    let received = tokio::select! {
                        biased;
                        _ = stopped(stop) => return Err(p.error(Code::Stopped)),
                        result = timeout_at(control_deadline, exchange) => match result {
                            Ok(result) => result.map_err(|_| p.error(Code::Transport))?,
                            Err(_) if Instant::now() >= deadline => return Err(p.financial_error(financial::admission_timeout(Some(&unknown)))),
                            Err(_) => return Err(p.error(Code::Transport)),
                        },
                    };
                    if !matches!(received.message(), Message::State { state: PublicState::Prepared }) {
                        // Existing execution, cancellation or closure must be
                        // recovered. Never interpret it as a new Execute permit.
                        return Err(p.error(Code::RecoveryRequired));
                    }
                    next_status = Instant::now() + heartbeat;
                },
            }
        }
    }
}
