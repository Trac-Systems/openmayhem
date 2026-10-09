use super::*;
use serde_json::Value;
use std::sync::atomic::Ordering;
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
        let concurrency = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
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
        self.end.get_or_insert_with(Instant::now);
        self.body.observed_at_ms = self.recorder.store.now_ms();
    }
    pub(crate) async fn verified(mut self, response: &Value) -> Result<()> {
        let end = self
            .end
            .ok_or_else(|| crate::invalid("conformance result not complete"))?;
        self.body.result_digest = digest("mayhem/proxy/conformance-result/v1", response)?;
        self.body.expires_at_ms = self
            .body
            .observed_at_ms
            .saturating_add(self.recorder.store.config.ttl_ms);
        self.body.assertions = assertions(self.body.class.streaming, self.json_schema, response);
        if !self.speed_invalid
            && self.concurrency == 1
            && self.recorder.generation.load(Ordering::Acquire) == self.generation
        {
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
                            first_output_ms: self
                                .first
                                .map(|t| t.duration_since(self.start).as_millis() as u64)
                                .unwrap_or(0),
                            total_ms: end.duration_since(self.start).as_millis() as u64,
                            local_concurrency: 1,
                            remote_cache: "unknown".into(),
                            remote_concurrency: "unknown".into(),
                        });
                    }
                }
            }
        }
        self.recorder.retain(self.body.clone()).await
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.recorder.active.fetch_sub(1, Ordering::AcqRel);
        self.recorder.generation.fetch_add(1, Ordering::AcqRel);
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
