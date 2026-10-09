//! Bounded signing recovery. Canonical I/O stays outside storage transactions;
//! journal retirement commits before capacity release and closure commits last.
use super::*;
use crate::attempts::{retirement::Retirement, Event, Journal, Phase};

pub struct RetirementPage {
    pub examined: usize,
    pub closed: usize,
    pub failed: usize,
    pub next_after: Option<Digest>,
    pub(crate) completed: Vec<(Digest, Digest)>,
}

fn finish(journal: &Journal, runtime: &Runtime, saved: &Retirement, now_ms: u64) -> Result<()> {
    require(
        saved.lease().route == runtime.route,
        "retirement belongs to another route",
    )?;
    runtime
        .capacity
        .release_retired(saved)
        .map_err(|_| invalid("retired capacity release failed"))?;
    let invocation = &saved.lease().work.invocation;
    let closure = saved
        .closure()
        .map_err(|_| invalid("retirement closure invalid"))?;
    let record = journal
        .get_attempt(invocation, saved.attempt())
        .map_err(|_| invalid("retirement journal unavailable"))?
        .ok_or_else(|| invalid("retirement journal missing"))?;
    if record.phase == Phase::Closed {
        return require(
            record.closure == Some(closure),
            "retirement closure differs",
        );
    }
    match journal.advance(
        invocation,
        record.generation,
        Event::Close(closure.clone()),
        now_ms.max(record.updated_at_ms),
    ) {
        Ok(_) => Ok(()),
        Err(_) => {
            let current = journal
                .get_attempt(invocation, saved.attempt())
                .map_err(|_| invalid("retirement journal unavailable"))?;
            require(
                current.is_some_and(|r| {
                    r.attempt == record.attempt
                        && r.phase == Phase::Closed
                        && r.closure == Some(closure)
                }),
                "retirement closure failed",
            )
        }
    }
}
impl ProviderNegotiation {
    pub(crate) async fn reconcile_signing(
        &self,
        runtime: Arc<Runtime>,
        lease: Digest,
        client: Arc<Client>,
        now_ms: u64,
    ) -> Result<bool> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider recovery storage busy"))?;
        let r = runtime.clone();
        let original = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let original = r
                .capacity
                .signing_intent(&lease)
                .map_err(|_| invalid("signing intention unavailable"))?;
            Ok::<_, Error>(original)
        })
        .await
        .map_err(|_| Error::Task)??;
        let Some(original) = original else {
            return Ok(false);
        };
        require(
            original.lease().route == runtime.route,
            "signing intention belongs to another route",
        )?;
        let observation = client.intent_state(original.buyer()).await?;
        if observation.status()? != intent::Status::Expired {
            return Ok(false);
        }
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider recovery storage busy"))?;
        let journal = self.journal.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            require(
                journal
                    .identity()
                    .map_err(|_| invalid("provider journal unavailable"))?
                    == *runtime.capacity.identity(),
                "provider recovery identity differs",
            )?;
            let saved = journal
                .retire_provider(&original, &observation, now_ms)
                .map_err(|_| invalid("provider retirement not committed"))?;
            finish(&journal, &runtime, &saved, now_ms)?;
            Ok(true)
        })
        .await
        .map_err(|_| Error::Task)?
    }

    /// A second bounded cursor is necessary: after capacity release, the lease
    /// index can no longer find an interrupted journal closure. No network reads.
    pub(crate) async fn reconcile_retirements(
        &self,
        runtime: Arc<Runtime>,
        after: Option<Digest>,
        limit: usize,
        now_ms: u64,
    ) -> Result<RetirementPage> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| invalid("provider recovery storage busy"))?;
        let journal = self.journal.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            require(
                journal
                    .identity()
                    .map_err(|_| invalid("provider journal unavailable"))?
                    == *runtime.capacity.identity(),
                "provider recovery identity differs",
            )?;
            let page = journal
                .recovery_page(after.as_ref(), limit)
                .map_err(|_| invalid("provider recovery page unavailable"))?;
            let mut result = RetirementPage {
                examined: page.records.len(),
                closed: 0,
                failed: 0,
                next_after: page.next_after,
                completed: Vec::new(),
            };
            for record in page.records {
                match journal.provider_retirement(&record.invocation, record.attempt) {
                    Ok(Some(saved)) if saved.lease().route == runtime.route => {
                        if finish(&journal, &runtime, &saved, now_ms).is_ok() {
                            result.closed += 1;
                            result.completed.push((
                                saved.lease().work.invocation.clone(),
                                saved.lease().id.clone(),
                            ));
                        } else {
                            result.failed += 1;
                        }
                    }
                    Err(_) => result.failed += 1,
                    _ => (),
                }
            }
            Ok(result)
        })
        .await
        .map_err(|_| Error::Task)?
    }
}
