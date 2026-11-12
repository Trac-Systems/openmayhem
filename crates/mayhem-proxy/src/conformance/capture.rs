use super::*;
use serde_json::Value;
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

/// This type is never deserialized. Callers can feed only already validated
/// endpoint events; completion belongs to the trusted final-verification path.
pub(crate) struct Capture {
    recorder: Arc<Recorder>,
    body: Body,
    start: Instant,
    first: Option<Instant>,
    end: Option<Instant>,
    native: Option<health::native::Capture>,
    generation: u64,
    concurrency: u64,
    active: bool,
    json_schema: bool,
    speed_invalid: bool,
}
impl Recorder {
    pub(crate) fn begin(
        self: &Arc<Self>,
        terms: &ProxySpendTerms,
        value: &Value,
    ) -> Result<Capture> {
        terms.validate().map_err(crate::invalid)?;
        require(
            mayhem_proto::endpoint_request_fingerprint(value) == terms.request_hash
                && terms.buyer_pubkey == self.store.config.tester.as_str(),
            "conformance original request differs",
        )?;
        self.capture(
            Subject::terms(terms)?,
            hash_value(&terms.connection_digest)?,
            hash_value(&terms.session_id)?,
            hash_value(&terms.request_hash)?,
            value,
            Provenance::GatewayObservation,
        )
    }
    fn capture(
        self: &Arc<Self>,
        subject: Subject,
        connection_digest: Digest,
        session: Digest,
        request_hash: Digest,
        value: &Value,
        provenance: Provenance,
    ) -> Result<Capture> {
        let class = Class::request(value)?;
        let native = if class.streaming {
            self.store.tokenizer.as_ref().and_then(|s| s.capture())
        } else {
            None
        };
        let mut activity = self
            .activity
            .lock()
            .map_err(|_| crate::invalid("conformance activity unavailable"))?;
        let generation = activity
            .generation
            .checked_add(1)
            .ok_or_else(|| crate::invalid("conformance activity exhausted"))?;
        activity.active += 1;
        activity.generation = generation;
        let concurrency = activity.active;
        Ok(Capture {
            recorder: self.clone(),
            body: Body {
                schema_version: 1,
                network: self.store.network.clone(),
                tester: self.store.config.tester.clone(),
                provenance,
                boot: self.store.boot.clone(),
                configuration: self.store.configuration.clone(),
                suite: SUITE.into(),
                subject,
                class,
                session,
                request_hash,
                connection_digest,
                result_digest: Digest::hash("pending", &[]),
                observed_at_ms: 0,
                expires_at_ms: 0,
                assertions: Vec::new(),
                speed: None,
            },
            start: Instant::now(),
            first: None,
            end: None,
            native,
            generation,
            concurrency,
            active: true,
            json_schema: value
                .pointer("/response_format/type")
                .or_else(|| value.pointer("/text/format/type"))
                == Some(&Value::String("json_schema".into())),
            speed_invalid: false,
        })
    }
    /// Takes a non-deserializable result of the actual decoder/capacity probe.
    /// Provider self-tests remain declared evidence, even with a valid signature.
    pub async fn retain_probe(
        self: &Arc<Self>,
        subject: Subject,
        completion: &crate::execution::probes::Conformance,
    ) -> Result<()> {
        require(
            subject.provider == self.store.config.tester
                && subject.provider == completion.provider
                && completion.network == self.store.network
                && subject.endpoint == completion.endpoint
                && subject.endpoint_contract == completion.contract
                && subject.recipe_hash == completion.recipe
                && subject.connection_revision == completion.connection_revision
                && completion.completed >= self.store.opened,
            "probe subject differs from validated adapter",
        )?;
        let now = self.store.now_ms();
        let age = completion
            .completed
            .elapsed()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        let observed = now.saturating_sub(age);
        let body = Body {
            schema_version: 1,
            network: self.store.network.clone(),
            tester: self.store.config.tester.clone(),
            provenance: Provenance::ProviderSelfTest,
            boot: self.store.boot.clone(),
            configuration: self.store.configuration.clone(),
            suite: SUITE.into(),
            subject,
            class: completion.class.clone(),
            session: completion.probe.clone(),
            request_hash: completion.request_hash.clone(),
            connection_digest: completion.connection.clone(),
            result_digest: completion.result.clone(),
            observed_at_ms: observed,
            expires_at_ms: observed.saturating_add(self.store.config.ttl_ms),
            assertions: completion.assertions.clone(),
            speed: None,
        };
        self.retain(body).await
    }
    async fn retain(self: &Arc<Self>, body: Body) -> Result<()> {
        let permit = self
            .writes
            .clone()
            .try_acquire_owned()
            .map_err(|_| crate::invalid("conformance storage busy"))?;
        self.retain_reserved(body, permit).await
    }
    async fn retain_reserved(
        self: &Arc<Self>,
        body: Body,
        permit: OwnedSemaphorePermit,
    ) -> Result<()> {
        let recorder = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let signed = recorder.signer.conformance(body)?;
            recorder.store.retain(signed)
        })
        .await
        .map_err(|_| crate::Error::Task)?
    }
}
impl Capture {
    pub(crate) fn delta(&mut self, value: &Value) {
        let now = Instant::now();
        if health::meaningful(value) {
            self.first.get_or_insert(now);
        }
        if let Some(native) = &mut self.native {
            native.delta(value, now)
        }
    }
    pub(crate) fn backpressure(&mut self) {
        self.speed_invalid = true;
    }
    pub(crate) fn complete_network(&mut self) {
        if self.end.is_none() {
            self.end = Some(Instant::now());
            self.body.observed_at_ms = self.recorder.store.now_ms();
            // Only overlapping execution invalidates this interval. Later
            // settlement/tokenization/storage must neither extend activity nor
            // allow a concurrent begin to race the generation check.
            self.end_activity();
        }
    }
    fn end_activity(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        if let Ok(mut activity) = self.recorder.activity.lock() {
            self.speed_invalid |= self.concurrency != 1 || activity.generation != self.generation;
            activity.active -= 1;
            activity.generation = activity.generation.saturating_add(1);
        } else {
            self.speed_invalid = true;
        }
    }
    /// Freeze verified metadata without keeping the response or waiting for
    /// optional workers. Four recorder permits bound both tails and writes.
    pub(crate) fn verified(mut self, response: &Value) -> Result<Verified> {
        let end = self
            .end
            .ok_or_else(|| crate::invalid("conformance result not complete"))?;
        let permit = self
            .recorder
            .writes
            .clone()
            .try_acquire_owned()
            .map_err(|_| crate::invalid("conformance storage busy"))?;
        self.body.result_digest = digest("mayhem/proxy/conformance-result/v1", response)?;
        self.body.expires_at_ms = self
            .body
            .observed_at_ms
            .saturating_add(self.recorder.store.config.ttl_ms);
        self.body.assertions = assertions(self.body.class.streaming, self.json_schema, response);
        Ok(Verified {
            recorder: self.recorder.clone(),
            body: self.body.clone(),
            native: if self.speed_invalid {
                None
            } else {
                self.native.take()
            },
            first_output_ms: self
                .first
                .map(|t| t.duration_since(self.start).as_millis() as u64)
                .unwrap_or(0),
            total_ms: end.duration_since(self.start).as_millis() as u64,
            permit,
        })
    }
}
/// Owned by the existing buyer task after delivering its financial outcome.
/// Contains no full response, and retains its permit until storage completes.
pub(crate) struct Verified {
    recorder: Arc<Recorder>,
    body: Body,
    native: Option<health::native::Capture>,
    first_output_ms: u64,
    total_ms: u64,
    permit: OwnedSemaphorePermit,
}
impl Verified {
    pub(crate) async fn retain(mut self) -> Result<()> {
        if let Some(native) = self.native.take().map(|n| n.finish()) {
            if let Some(e) = native.await {
                let us = e
                    .last
                    .duration_since(e.first)
                    .as_micros()
                    .min(u64::MAX as u128) as u64;
                let c = &self.recorder.store.config;
                if e.tokens >= c.minimum_interval_tokens && us >= c.minimum_interval_us {
                    self.body.speed = Some(Speed {
                        tokenizer: e.digest,
                        interval_tokens: e.tokens,
                        interval_us: us,
                        samples: 1,
                        first_output_ms: self.first_output_ms,
                        total_ms: self.total_ms,
                        local_concurrency: 1,
                        remote_cache: "unknown".into(),
                        remote_concurrency: "unknown".into(),
                    });
                }
            }
        }
        self.recorder.retain_reserved(self.body, self.permit).await
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.end_activity();
    }
}
pub(crate) fn assertions(streaming: bool, schema: bool, response: &Value) -> Vec<Assertion> {
    let mut result = vec![Assertion::ValidEndpointOutput];
    if streaming {
        result.push(Assertion::ValidatedStream);
    }
    let choices = response["choices"].as_array();
    let output = response["output"].as_array();
    let tools = choices.is_some_and(|v| {
        v.iter().any(|c| {
            c["message"]["tool_calls"]
                .as_array()
                .is_some_and(|t| !t.is_empty())
        })
    }) || output.is_some_and(|v| v.iter().any(|v| v["type"] == "function_call"));
    if tools {
        result.push(Assertion::ValidatedToolCall);
    }
    let text = choices.is_some_and(|v| {
        !v.is_empty()
            && v.iter().all(|c| {
                c["message"]["refusal"].is_null()
                    && c["message"]["content"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
            })
    }) || output.is_some_and(|v| {
        v.iter().any(|v| {
            v["type"] == "message"
                && v["content"].as_array().is_some_and(|p| {
                    p.iter().any(|p| {
                        p["type"] == "output_text"
                            && p["text"].as_str().is_some_and(|s| !s.is_empty())
                    })
                })
        })
    });
    if schema && text && !tools {
        result.push(Assertion::ValidatedJsonSchema);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::conformance::tests::{body, config, directory, network, signer};
    use serde_json::json;

    fn recorder() -> (tempfile::TempDir, Arc<Recorder>) {
        let dir = directory();
        let store =
            Arc::new(Store::open(&dir.path().join("records"), network(), config()).unwrap());
        let recorder = Arc::new(Recorder::new(store, signer()).unwrap());
        (dir, recorder)
    }
    fn capture(recorder: &Arc<Recorder>) -> Capture {
        let b = body(&recorder.store);
        recorder.capture(
            b.subject,
            b.connection_digest,
            b.session,
            b.request_hash,
            &json!({"model":"public", "messages":[{"role":"user","content":"hello"}], "stream":true}),
            Provenance::GatewayObservation,
        ).unwrap()
    }
    fn response() -> Value {
        json!({"choices":[{"message":{"content":"one two three"}}]})
    }

    #[tokio::test]
    async fn verified_tails_are_bounded_and_release_activity_before_recording() {
        let (_dir, recorder) = recorder();
        let mut tails = Vec::new();
        for _ in 0..4 {
            let mut sample = capture(&recorder);
            sample.complete_network();
            assert!(
                !sample.speed_invalid,
                "pending recording is not overlapping execution"
            );
            tails.push(sample.verified(&response()).unwrap());
        }
        assert_eq!(recorder.activity.lock().unwrap().active, 0);
        assert_eq!(recorder.writes.available_permits(), 0);
        let mut skipped = capture(&recorder);
        skipped.complete_network();
        assert!(skipped.verified(&response()).is_err());
        let b = &tails[0].body;
        assert!(matches!(
            recorder.store.lookup(&b.subject, &b.class).unwrap(),
            Lookup::Missing
        ));
        drop(tails.pop());
        assert_eq!(recorder.writes.available_permits(), 1);
        tails.pop().unwrap().retain().await.unwrap();
        assert_eq!(recorder.writes.available_permits(), 2);
        drop(tails);
        assert_eq!(recorder.writes.available_permits(), 4);
    }

    #[test]
    fn overlapping_or_abandoned_execution_disqualifies_speed_but_later_work_does_not() {
        let (_dir, recorder) = recorder();
        let mut first = capture(&recorder);
        let mut second = capture(&recorder);
        first.complete_network();
        second.complete_network();
        assert!(first.speed_invalid && second.speed_invalid);
        let mut alone = capture(&recorder);
        alone.complete_network();
        let mut later = capture(&recorder);
        later.complete_network();
        assert!(!alone.speed_invalid && !later.speed_invalid);
        let mut interrupted = capture(&recorder);
        drop(capture(&recorder));
        interrupted.complete_network();
        assert!(interrupted.speed_invalid);
        drop((first, second, alone, later, interrupted));
        assert_eq!(recorder.activity.lock().unwrap().active, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_recording_preserves_network_time_duration_and_expiry() {
        let (_dir, recorder) = recorder();
        let mut sample = capture(&recorder);
        tokio::time::advance(std::time::Duration::from_millis(100)).await;
        sample.complete_network();
        let observed = sample.body.observed_at_ms;
        tokio::time::advance(std::time::Duration::from_secs(90)).await;
        sample.complete_network();
        let verified = sample.verified(&response()).unwrap();
        assert_eq!(verified.body.observed_at_ms, observed);
        assert_eq!(verified.total_ms, 100);
        let subject = verified.body.subject.clone();
        let class = verified.body.class.clone();
        let expires = observed + recorder.store.config.ttl_ms;
        assert_eq!(verified.body.expires_at_ms, expires);
        recorder.store.effective_time(expires + 1);
        verified.retain().await.unwrap();
        assert!(matches!(
            recorder.store.lookup(&subject, &class).unwrap(),
            Lookup::Expired
        ));
        assert_eq!(recorder.writes.available_permits(), 4);
    }

    #[tokio::test]
    async fn failed_optional_write_releases_its_permit() {
        let (_dir, recorder) = recorder();
        let mut sample = capture(&recorder);
        sample.complete_network();
        let mut verified = sample.verified(&response()).unwrap();
        // Force signing failure without touching any financial record.
        verified.body.assertions.clear();
        assert!(verified.retain().await.is_err());
        assert_eq!(recorder.writes.available_permits(), 4);
        assert_eq!(recorder.activity.lock().unwrap().active, 0);
    }

    #[test]
    fn stalled_optional_storage_stays_bounded_and_owned_until_drain() {
        let (_dir, recorder) = recorder();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            // Occupy the blocking executor to stall actual conformance writes
            // without introducing a storage mock or waiting on external I/O.
            let (started, ready) = std::sync::mpsc::channel();
            let (release, blocked) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                let _ = blocked.recv();
            });
            ready.recv().unwrap();
            let mut tasks = tokio::task::JoinSet::new();
            for _ in 0..4 {
                let mut sample = capture(&recorder);
                sample.complete_network();
                let verified = sample.verified(&response()).unwrap();
                tasks.spawn(verified.retain());
            }
            tokio::task::yield_now().await;
            assert!(tasks.try_join_next().is_none());
            assert_eq!(tasks.len(), 4);
            assert_eq!(recorder.writes.available_permits(), 0);
            let mut skipped = capture(&recorder);
            skipped.complete_network();
            assert!(skipped.verified(&response()).is_err());
            release.send(()).unwrap();
            blocker.await.unwrap();
            while let Some(result) = tasks.join_next().await {
                result.unwrap().unwrap();
            }
            assert_eq!(recorder.writes.available_permits(), 4);
        });
    }
}
