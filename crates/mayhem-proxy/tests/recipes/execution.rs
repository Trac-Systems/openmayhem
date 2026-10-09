use super::*;
#[path = "../support/recipes.rs"]
mod recipe_fixture;
fn custom(base: &str, recipe: mayhem_proxy::recipe::Signed) -> Fixture {
    custom_limit(base, recipe, 1024 * 1024)
}
fn custom_limit(
    base: &str,
    recipe: mayhem_proxy::recipe::Signed,
    response_bytes: usize,
) -> Fixture {
    let mut fixture = Fixture::with_profile(
        base,
        recipe.recipe.endpoint,
        128 * 1024 * 1024,
        "http_status",
    );
    let mut snapshot = fixture.adapter.snapshot();
    snapshot.limits.response_bytes = response_bytes;
    fixture.adapter = Arc::new(
        Adapter::restore(snapshot)
            .unwrap()
            .with_recipe(recipe)
            .unwrap(),
    );
    let pool = Arc::new(
        Pool::new(
            env!("CARGO_BIN_EXE_mayhem-proxy-worker"),
            fixture._work.path(),
            PoolLimits {
                max_children: 2,
                max_buffer_bytes: 64 * 1024 * 1024,
                startup_timeout: Duration::from_secs(5),
                processing_timeout: Duration::from_secs(3),
            },
        )
        .unwrap(),
    );
    fixture.executor = Executor::new(
        fixture.connection.clone(),
        fixture.adapter.clone(),
        pool,
        Arc::new(Storage::new(fixture.journal.clone(), 8).unwrap()),
    )
    .unwrap();
    fixture
}
#[tokio::test]
async fn four_custom_interfaces_use_real_worker_metering_and_original_recovery() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let recipe = recipe_fixture::recipe(endpoint);
        let sample = recipe.recipe.fixtures[0].clone();
        let backend = backend(200, sample.upstream_response.clone(), Duration::ZERO).await;
        let fixture = custom(&backend.base, recipe.clone());
        let bytes = serde_json::to_vec(&sample.request).unwrap();
        let record = fixture.prepare(901, &bytes, ProxyRail::Tap);
        let expected = recipe_fixture::base(endpoint)
            .prepare_json(&bytes)
            .unwrap()
            .decode_json(
                sample.normalized_response,
                &format!("proxy_{}", record.invocation.as_str()),
                record.created_at_ms / 1000,
            )
            .unwrap();
        let result = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap();
        assert_eq!(result.reply.body, expected.body);
        assert_eq!(
            serde_json::to_value(result.reply.observed_usage).unwrap(),
            serde_json::to_value(expected.observed_usage).unwrap()
        );
        assert_eq!(result.attempt.binding.rail, ProxyRail::Tap);
        assert_eq!(
            result.attempt.binding.recipe_digest,
            *fixture.adapter.recipe_hash()
        );
        assert_eq!(
            result.attempt.binding.request_hash.as_str(),
            mayhem_proto::endpoint_request_fingerprint(&sample.request)
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        let mut mapped = sample.upstream_request;
        mapped["engine"] = json!("upstream-model");
        if endpoint == ProxyEndpoint::Responses {
            mapped["retain"] = json!(false);
        }
        assert_eq!(backend.bodies.lock().unwrap()[0], mapped);
        assert!(matches!(
            fixture
                .executor
                .execute_json(&record.invocation, &bytes, &Cancellation::default())
                .await,
            Err(Error::RecoveryRequired)
        ));
        let recovered = fixture
            .journal
            .recover(&record.invocation, record.attempt)
            .unwrap();
        let original = Adapter::restore(recovered.acceptance.unwrap().snapshot.adapter).unwrap();
        assert_eq!(original.recipe_hash(), fixture.adapter.recipe_hash());
        assert!(recovered.result.is_some());
        assert!(recovered.record.closure.is_none());
        let mut revised = recipe.recipe;
        revised.revision += 1;
        let changed = Adapter::restore(fixture.adapter.snapshot())
            .unwrap()
            .with_recipe(recipe_fixture::sign(revised))
            .unwrap();
        assert_ne!(changed.recipe_hash(), original.recipe_hash());
        assert!(!changed
            .prepare_json(&bytes)
            .unwrap()
            .matches_binding(&record.binding));
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "recovery never replays POST after recipe changes"
        );
    }
}
fn tool_request(recipe: &mayhem_proxy::recipe::Signed) -> Value {
    let mut body = recipe.recipe.fixtures[0].request.clone();
    body["tools"] = json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}}}]);
    body["tool_choice"] = json!("required");
    body
}
fn tool_response(recipe: &mayhem_proxy::recipe::Signed, args: &str) -> Value {
    let mut body = recipe.recipe.fixtures[0].upstream_response.clone();
    body["results"][0]["stop"] = json!("tools");
    body["results"][0]["text"] = Value::Null;
    body["results"][0]["calls"] =
        json!([{"id":"call1","type":"function","function":{"name":"lookup","arguments":args}}]);
    body
}
#[tokio::test]
async fn worker_checks_original_tool_schema_after_mapping_and_keeps_exact_arguments() {
    for (args, valid) in [
        ("{ \"key\": \"abc\" }", true),
        ("{\"key\":1}", false),
        ("{\"key\":\"a\",\"extra\":true}", false),
    ] {
        let recipe = recipe_fixture::recipe(ProxyEndpoint::Chat);
        let bytes = serde_json::to_vec(&tool_request(&recipe)).unwrap();
        let backend = backend(200, tool_response(&recipe, args), Duration::ZERO).await;
        let fixture = custom(&backend.base, recipe);
        let record = fixture.prepare(902, &bytes, ProxyRail::Fiat);
        let result = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await;
        if valid {
            assert_eq!(
                result.unwrap().reply.body["choices"][0]["message"]["tool_calls"][0]["function"]
                    ["arguments"],
                args
            );
        } else {
            assert!(
                matches!(result,Err(Error::Decoder(mayhem_proxy::worker::Error::Upstream(f))) if f.code==Code::UpstreamProtocol)
            );
            assert!(fixture
                .journal
                .recover(&record.invocation, record.attempt)
                .unwrap()
                .result
                .is_none());
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn invalid_schema_fails_in_worker_before_post_and_custom_error_is_not_retry_authority() {
    let recipe = recipe_fixture::recipe(ProxyEndpoint::Chat);
    let mut body = tool_request(&recipe);
    body["tools"][0]["function"]["parameters"] = json!({"$ref":"https://forbidden.invalid/schema"});
    let backend = backend(200, tool_response(&recipe, "{}"), Duration::ZERO).await;
    let fixture = custom(&backend.base, recipe);
    let bytes = serde_json::to_vec(&body).unwrap();
    let record = fixture.prepare(903, &bytes, ProxyRail::Fiat);
    assert!(fixture
        .executor
        .execute_json(&record.invocation, &bytes, &Cancellation::default())
        .await
        .is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    for state in ["busy", "bad", "future"] {
        let recipe = recipe_fixture::recipe(ProxyEndpoint::Chat);
        let bytes = serde_json::to_vec(&recipe.recipe.fixtures[0].request).unwrap();
        let server = super::backend(
            200,
            json!({"state":state,"fault":{"message":"PRIVATE_CREDENTIAL_should_never_leave"}}),
            Duration::ZERO,
        )
        .await;
        let fixture = custom(&server.base, recipe);
        let record = fixture.prepare(904, &bytes, ProxyRail::Fiat);
        let error = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("PRIVATE_CREDENTIAL"));
        assert!(
            matches!(error,Error::Decoder(mayhem_proxy::worker::Error::Upstream(f)) if f.execution==Execution::Unknown && matches!(f.retry_advice(),mayhem_proxy::connector::failure::RetryAdvice::RecoverSameAttempt))
        );
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn custom_structured_output_and_decision_results_keep_common_constraints() {
    for text in ["{\"ok\":true}", "{\"ok\":\"yes\"}"] {
        let recipe = recipe_fixture::recipe(ProxyEndpoint::Chat);
        let mut body = recipe.recipe.fixtures[0].request.clone();
        body["response_format"] = json!({"type":"json_schema","json_schema":{"name":"result","schema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}}});
        let mut raw = recipe.recipe.fixtures[0].upstream_response.clone();
        raw["results"][0]["text"] = json!(text);
        let server = backend(200, raw, Duration::ZERO).await;
        let fixture = custom(&server.base, recipe);
        let bytes = serde_json::to_vec(&body).unwrap();
        let record = fixture.prepare(905, &bytes, ProxyRail::Fiat);
        let result = fixture
            .executor
            .execute_json(&record.invocation, &bytes, &Cancellation::default())
            .await;
        assert_eq!(result.is_ok(), text == "{\"ok\":true}");
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    }
    for bad in [
        json!({"type":"choice","choice":"unknown","probabilities":{"yes":0.75,"no":0.25}}),
        json!({"type":"choice","choice":"yes","probabilities":{"yes":1.75,"no":0.25}}),
    ] {
        let recipe = recipe_fixture::recipe(ProxyEndpoint::Decisions);
        let bytes = serde_json::to_vec(&recipe.recipe.fixtures[0].request).unwrap();
        let mut raw = recipe.recipe.fixtures[0].upstream_response.clone();
        raw["labels"]["q"] = bad;
        let server = backend(200, raw, Duration::ZERO).await;
        let fixture = custom(&server.base, recipe);
        let record = fixture.prepare(906, &bytes, ProxyRail::Fiat);
        assert!(matches!(
            fixture
                .executor
                .execute_json(&record.invocation, &bytes, &Cancellation::default())
                .await,
            Err(Error::Endpoint(_))
        ));
        assert!(fixture
            .journal
            .recover(&record.invocation, record.attempt)
            .unwrap()
            .result
            .is_none());
    }
}

#[tokio::test]
async fn worker_enforces_adapter_byte_limit_before_emitting_expanded_projection() {
    use mayhem_proxy::recipe::{Field, Projection};
    let mut recipe = recipe_fixture::recipe(ProxyEndpoint::Chat).recipe;
    let Projection::Object { fields } = &mut recipe.response else {
        panic!("object")
    };
    for n in 0..5 {
        let name = format!("copied_{n}");
        fields.insert(
            name.clone(),
            Field {
                optional: false,
                value: Projection::Copy {
                    path: vec!["results".into()],
                },
            },
        );
        recipe.fixtures[0].normalized_response[&name] =
            recipe.fixtures[0].upstream_response["results"].clone();
    }
    let mut raw = recipe.fixtures[0].upstream_response.clone();
    raw["results"][0]["text"] = json!("a".repeat(300));
    let bytes = serde_json::to_vec(&recipe.fixtures[0].request).unwrap();
    let server = backend(200, raw, Duration::ZERO).await;
    let fixture = custom_limit(&server.base, recipe_fixture::sign(recipe), 1024);
    let record = fixture.prepare(907, &bytes, ProxyRail::Fiat);
    assert!(
        matches!(fixture.executor.execute_json(&record.invocation,&bytes,&Cancellation::default()).await,Err(Error::Decoder(mayhem_proxy::worker::Error::Upstream(f))) if f.code==Code::UpstreamProtocol)
    );
    assert!(fixture
        .journal
        .recover(&record.invocation, record.attempt)
        .unwrap()
        .result
        .is_none());
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
}
