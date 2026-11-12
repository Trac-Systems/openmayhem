use super::*;

#[tokio::test]
async fn public_assistant_message_roundtrips_through_paid_tool_followup() {
    let f = Fixture::start().await;
    *f.harness.backend_response.lock().unwrap() = Some(json!({
        "id":"private-tool-reply","object":"chat.completion",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,
            "reasoning":null,"reasoning_content":null,"refusal":null,
            "tool_calls":[{"id":"call_1","type":"function","function":{"name":"health_echo","arguments":"{}"}}]},
            "finish_reason":"tool_calls"}]
    }));
    let mut body = f.body();
    body["tools"] = json!([{"type":"function","function":{"name":"health_echo",
        "parameters":{"type":"object","properties":{},"additionalProperties":false}}}]);
    body["tool_choice"] = json!("required");
    let (status, _, first) = f
        .post("tool-first", body.clone(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let client_message = first["choices"][0]["message"].clone();
    for field in ["reasoning", "reasoning_content", "refusal"] {
        assert_eq!(client_message.get(field), Some(&Value::Null));
    }
    body["messages"].as_array_mut().unwrap().extend([
        client_message.clone(),
        json!({"role":"tool","tool_call_id":client_message["tool_calls"][0]["id"],"content":"ok"}),
    ]);
    body["tool_choice"] = json!("none");
    *f.harness.backend_response.lock().unwrap() = None;
    let (status, headers, second) = f
        .post("tool-followup", body.clone(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["choices"][0]["message"]["content"], "hello");
    assert_eq!(f.harness.backend_calls(), 2);

    let parsed =
        proxy_request::Request::parse(ProxyEndpoint::Chat, body.clone(), &f.runtime.policy)
            .unwrap()
            .unwrap();
    let mut dispatched = f.harness.backend_request.lock().unwrap().clone().unwrap();
    assert_eq!(dispatched["model"], "upstream-model");
    dispatched["model"] = body["model"].clone();
    assert_eq!(
        dispatched,
        serde_json::from_slice::<Value>(parsed.provider_request()).unwrap()
    );
    assert_eq!(parsed.provider_value()["messages"], body["messages"]);
    assert_eq!(
        dispatched["messages"][1]["tool_calls"],
        client_message["tool_calls"]
    );
    assert_eq!(dispatched["messages"][1]["content"], Value::Null);
    let job_id = headers["x-mayhem-job-id"].to_str().unwrap();
    let job = load(&f.state, job_id).await.unwrap().unwrap();
    assert_eq!(
        job.proxy.as_ref().unwrap().terms().unwrap().request_hash,
        mayhem_proto::endpoint_request_fingerprint(&dispatched)
    );

    let (status, _, replay) = f
        .post("tool-followup", body.clone(), "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, second);
    let mut absent = body;
    for field in ["reasoning", "reasoning_content", "refusal"] {
        absent["messages"][1].as_object_mut().unwrap().remove(field);
    }
    let (status, _, replay) = f
        .post("tool-followup", absent, "owner-fixture-key", false)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{replay}");
    assert_eq!(
        f.harness.backend_calls(),
        2,
        "replay or changed raw identity must not dispatch again"
    );
    f.stop().await;
}

#[tokio::test]
async fn resolver_assistant_nulls_preserve_external_raw_correlation_and_paid_execution_hash() {
    let _case = estimation::ESTIMATE_CASE.lock().await;
    let mut f = Fixture::start_with(ProxyEndpoint::Chat, ProxyRail::Fiat).await;
    let mut input = resolver::start(&f);
    input["request"]["messages"] = json!([
        {"role":"assistant","content":"Earlier answer.","reasoning":null,"refusal":null},
        {"role":"user","content":"Continue."}
    ]);
    let expected = input["request"].clone();
    let (status, resolved) = resolver::send(f.router.clone(), input, "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["status"], "selected", "{resolved}");
    assert_eq!(
        resolved["request_content_digest"],
        retail_request_content_digest(&expected).unwrap()
    );
    let selected = resolved["selection"]["request"].clone();
    assert_eq!(selected["messages"], expected["messages"]);
    let (status, _, estimate) =
        estimation::estimate(&f, selected.clone(), "owner-fixture-key").await;
    assert_eq!(status, StatusCode::OK, "{estimate}");
    for field in ["request_hash", "request_content_digest"] {
        assert_eq!(estimate[field], resolved["selection"]["estimate"][field]);
    }
    let mut provider_body = selected.clone();
    provider_body.as_object_mut().unwrap().remove("proxy");
    // Existing external API contract hashes the original provider body, while
    // the separate paid request_hash binds normalized execution bytes.
    assert_eq!(
        estimate["request_content_digest"],
        retail_request_content_digest(&provider_body).unwrap()
    );
    estimation::assert_no_purchase(&mut f).await;
    let (status, headers, response) = f
        .post(
            "resolved-assistant-history",
            selected.clone(),
            "owner-fixture-key",
            false,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let job = load(&f.state, headers["x-mayhem-job-id"].to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        estimate["request_hash"],
        job.proxy.as_ref().unwrap().terms().unwrap().request_hash
    );
    let mut dispatched = f.harness.backend_request.lock().unwrap().clone().unwrap();
    dispatched["model"] = selected["model"].clone();
    assert_ne!(
        estimate["request_content_digest"],
        retail_request_content_digest(&dispatched).unwrap()
    );
    assert_eq!(
        estimate["request_hash"],
        mayhem_proto::endpoint_request_fingerprint(&dispatched)
    );
    f.stop().await;
}
