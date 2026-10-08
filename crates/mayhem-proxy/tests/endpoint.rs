use mayhem_proto::{endpoint_family_contract_template, proxy::ProxyEndpoint};
use mayhem_proxy::{
    connector::failure::Code,
    endpoint::{Adapter, Error, Limits},
};
use serde_json::{json, Value};

fn adapter(endpoint: ProxyEndpoint) -> Adapter {
    let family = match endpoint {
        ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
        ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
        ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
        ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
    };
    Adapter::new(
        endpoint,
        endpoint_family_contract_template(family).unwrap(),
        "private-model".into(),
        Limits {
            request_bytes: 1024 * 1024,
            response_bytes: 1024 * 1024,
            choices: 16,
            tools: 32,
            questions: 32,
            decision_options: 64,
        },
    )
    .unwrap()
}
fn chat() -> Value {
    json!({"model":"public-model","messages":[{"role":"user","content":"hello"}]})
}
fn answer() -> Value {
    json!({"id":"upstream_1","object":"chat.completion","model":"private-model","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":2,"total_tokens":11}})
}
fn tools() -> Value {
    json!([{ "type":"function","function":{"name":"read_file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}])
}
fn call() -> Value {
    json!({"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{ \"path\": \"src/game.js\" }"}})
}

#[test]
fn preserves_text_refusals_reasoning_and_arguments_without_vendor_authority() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let request = adapter
        .prepare_json(&serde_json::to_vec(&chat()).unwrap())
        .unwrap();
    let mut upstream = answer();
    upstream["mayhem"] = json!({"receipt":{"paid":true},"provider":"private-host"});
    upstream["cost"] = json!(9999);
    upstream["choices"][0]["message"]["refusal"] = json!("I cannot help with that.");
    upstream["choices"][0]["message"]["reasoning_content"] = json!("provider reasoning");
    let reply = request.decode_json(upstream, "public_1", 100).unwrap();
    assert_eq!(reply.body["id"], "public_1");
    assert_eq!(reply.body["model"], "public-model");
    assert_eq!(
        reply.body["choices"][0]["message"]["refusal"],
        "I cannot help with that."
    );
    assert_eq!(reply.reported_usage.unwrap().input_tokens, 9);
    for field in ["mayhem", "cost", "usage"] {
        assert!(reply.body.get(field).is_none());
    }
    assert!(!format!("{request:?}").contains("hello"));

    let mut body = chat();
    body["tools"] = tools();
    body["tool_choice"] = json!("required");
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let mut upstream = answer();
    upstream["choices"][0]["message"]["tool_calls"] = json!([call()]);
    upstream["choices"][0]["finish_reason"] = json!("tool_calls");
    let reply = request.decode_json(upstream, "public_2", 101).unwrap();
    assert_eq!(
        reply.body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        call()["function"]["arguments"]
    );
}

#[test]
fn rejects_truncated_choices_undeclared_tools_bad_arguments_and_duplicate_ids() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let mut body = chat();
    body["tools"] = tools();
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    for (pointer, bad) in [
        ("/choices/0/finish_reason", Value::Null),
        ("/choices/0/finish_reason", json!("vendor-success")),
        ("/choices/0/index", json!(1)),
        ("/choices/0/message/role", json!("user")),
        ("/choices/0/message/content", json!({"not":"text"})),
    ] {
        let mut response = answer();
        *response.pointer_mut(pointer).unwrap() = bad;
        assert!(request.decode_json(response, "id", 0).is_err(), "{pointer}");
    }
    for modify in 0..6 {
        let mut response = answer();
        let mut c = call();
        match modify {
            0 => c["function"]["name"] = json!("delete_everything"),
            1 => c["function"]["arguments"] = json!("{\"path\":"),
            2 => c["function"]["arguments"] = json!("[]"),
            3 => c["id"] = json!(""),
            _ => (),
        }
        response["choices"][0]["message"]["tool_calls"] = if modify == 4 {
            json!([c.clone(), c])
        } else {
            json!([c])
        };
        response["choices"][0]["finish_reason"] =
            json!(if modify == 5 { "stop" } else { "tool_calls" });
        assert!(
            request.decode_json(response, "id", 0).is_err(),
            "case {modify}"
        );
    }
    let mut response = answer();
    response["choices"] = json!([]);
    assert!(request.decode_json(response, "id", 0).is_err());
}

#[test]
fn enforced_tool_selection_and_incomplete_or_refusal_outcomes_are_distinct() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let mut body = chat();
    body["tools"] = tools();
    body["tool_choice"] = json!("required");
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    assert!(request.decode_json(answer(), "id", 0).is_err());
    for finish in ["length", "content_filter"] {
        let mut response = answer();
        response["choices"][0]["finish_reason"] = json!(finish);
        assert_eq!(
            request.decode_json(response, "id", 0).unwrap().body["choices"][0]["finish_reason"],
            finish
        );
    }
    body["tool_choice"] = json!("none");
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let mut response = answer();
    response["choices"][0]["finish_reason"] = json!("tool_calls");
    response["choices"][0]["message"]["tool_calls"] = json!([call()]);
    assert!(request.decode_json(response, "id", 0).is_err());
}

#[test]
fn reported_usage_rejects_malformed_and_contradictory_counts_without_trusting_them() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let request = adapter
        .prepare_json(&serde_json::to_vec(&chat()).unwrap())
        .unwrap();
    for usage in [
        json!({"prompt_tokens":-1,"completion_tokens":1}),
        json!({"prompt_tokens":9,"completion_tokens":2,"total_tokens":10}),
        json!({"prompt_tokens":9,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":10}}),
        json!({"prompt_tokens":9,"completion_tokens":2,"completion_tokens_details":{"reasoning_tokens":3}}),
        json!({"prompt_tokens":9.5,"completion_tokens":2}),
        json!({"prompt_tokens":9,"completion_tokens":2,"prompt_tokens_details":"wrong"}),
    ] {
        let mut response = answer();
        response["usage"] = usage;
        assert!(request.decode_json(response, "id", 0).is_err());
    }
    let mut response = answer();
    response["usage"] = Value::Null;
    assert!(request
        .decode_json(response, "id", 0)
        .unwrap()
        .reported_usage
        .is_none());
    let mut response = answer();
    response["usage"]["prompt_tokens_details"] = json!({"cached_tokens":5});
    response["usage"]["completion_tokens_details"] = json!({"reasoning_tokens":1});
    let reply = request.decode_json(response, "id", 0).unwrap();
    assert_eq!(reply.reported_usage.unwrap().cached_input_tokens, Some(5));
    assert!(
        reply.body.get("usage").is_none(),
        "upstream counts are not buyer usage"
    );
}

#[test]
fn requests_are_contract_bound_and_never_silently_downgraded() {
    let adapter = adapter(ProxyEndpoint::Chat);
    for field in ["stream", "store", "background"] {
        let mut body = chat();
        body[field] = json!(true);
        assert!(
            adapter
                .prepare_json(&serde_json::to_vec(&body).unwrap())
                .is_err(),
            "{field}"
        );
    }
    let mut body = chat();
    body["unknown_secret_field"] = json!("secret-data");
    let error = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap_err();
    assert!(!format!("{error:?}").contains("secret-data"));
    assert!(!format!("{error:?}").contains("unknown_secret_field"));
    let a = adapter
        .prepare_json(&serde_json::to_vec(&chat()).unwrap())
        .unwrap();
    let b = adapter
        .prepare_json(br#"{"messages":[{"content":"hello","role":"user"}],"model":"public-model"}"#)
        .unwrap();
    assert_eq!(a.request_hash(), b.request_hash());
    let mut changed = chat();
    changed["messages"][0]["content"] = json!("changed");
    let c = adapter
        .prepare_json(&serde_json::to_vec(&changed).unwrap())
        .unwrap();
    assert_ne!(a.request_hash(), c.request_hash());
    let err = adapter
        .prepare_json(&vec![b' '; 1024 * 1024 + 1])
        .unwrap_err();
    assert!(matches!(err,Error::Request(f) if f.code==Code::RequestTooLarge));
}

#[test]
fn completions_and_stateless_responses_keep_their_public_shapes() {
    let adapter = adapter(ProxyEndpoint::Completions);
    let request = adapter
        .prepare_json(br#"{"model":"public-model","prompt":"hello"}"#)
        .unwrap();
    let response = json!({"id":"u","object":"text_completion","choices":[{"index":0,"text":" world","finish_reason":"stop"}]});
    let reply = request.decode_json(response, "p", 7).unwrap();
    assert_eq!(reply.body["choices"][0]["text"], " world");
    let adapter = crate::adapter(ProxyEndpoint::Responses);
    let request = adapter
        .prepare_json(br#"{"model":"public-model","input":"hello"}"#)
        .unwrap();
    let response = json!({"id":"u","object":"response","status":"completed","output":[{"type":"message","id":"m","role":"assistant","status":"completed","content":[{"type":"output_text","text":"world","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}});
    let reply = request.decode_json(response.clone(), "p", 7).unwrap();
    assert_eq!(reply.body["created_at"], 7);
    assert_eq!(reply.body["output"][0]["content"][0]["text"], "world");
    for status in ["queued", "in_progress", "failed", "cancelled"] {
        let mut bad = response.clone();
        bad["status"] = json!(status);
        assert!(request.decode_json(bad, "p", 7).is_err());
    }
    let mut incomplete = response;
    incomplete["status"] = json!("incomplete");
    assert!(request.decode_json(incomplete.clone(), "p", 7).is_err());
    incomplete["incomplete_details"] = json!({"reason":"max_output_tokens"});
    assert_eq!(
        request.decode_json(incomplete, "p", 7).unwrap().body["status"],
        "incomplete"
    );
}

#[test]
fn response_function_calls_are_checked_and_vendor_control_fields_removed() {
    let adapter = adapter(ProxyEndpoint::Responses);
    let request=adapter.prepare_json(&serde_json::to_vec(&json!({"model":"m","input":"hi","tools":[{"type":"function","name":"read_file","parameters":{"type":"object"}}]})).unwrap()).unwrap();
    let response = json!({"object":"response","status":"completed","output":[{"id":"i","type":"function_call","call_id":"c","name":"read_file","arguments":"{}","status":"completed","vendor_private":"secret"}]});
    let reply = request.decode_json(response.clone(), "p", 0).unwrap();
    assert!(reply.body["output"][0].get("vendor_private").is_none());
    let mut bad = response;
    bad["output"][0]["arguments"] = json!("null");
    assert!(request.decode_json(bad, "p", 0).is_err());
}

#[test]
fn decisions_validate_every_question_type_exact_ids_labels_probabilities_and_scores() {
    let adapter = adapter(ProxyEndpoint::Decisions);
    let body = json!({"model":"decisions","state":"An ordinary text","questions":{"topic":{"type":"choice","instructions":"topic","criteria":{"a":"A","b":"B","c":"C"}},"grade":{"type":"score","instructions":"grade","criteria":["low","high"]},"urgent":{"type":"noul","instructions":"urgent?"}}});
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let response = json!({"answers":{"topic":{"type":"choice","choice":"b","probabilities":{"a":0.3333,"b":0.3333,"c":0.3333},"confidence":0.5},"grade":{"type":"score","score":0.8,"probabilities":{"0":0.2,"1":0.8}},"urgent":{"type":"noul","noul":0.75,"action":{"act_probability":0.9}}},"usage":{"input_tokens":21,"output_tokens":0}});
    let reply = request.decode_json(response.clone(), "p", 0).unwrap();
    assert_eq!(reply.body["answers"]["grade"]["legend"]["1"], "high");
    for (pointer, bad) in [
        ("/answers/topic/choice", json!("invented")),
        ("/answers/topic/probabilities/a", json!(1.)),
        ("/answers/grade/score", json!(2.)),
        ("/answers/urgent/noul", json!(-0.1)),
        ("/answers/topic/type", json!("score")),
        ("/answers/urgent/action/act_probability", json!(1.1)),
    ] {
        let mut invalid = response.clone();
        *invalid.pointer_mut(pointer).unwrap() = bad;
        assert!(request.decode_json(invalid, "p", 0).is_err(), "{pointer}");
    }
    let mut missing = response.clone();
    missing["answers"].as_object_mut().unwrap().remove("urgent");
    assert!(request.decode_json(missing, "p", 0).is_err());
    let mut extra = response;
    extra["answers"]["extra"] = json!({"type":"noul","noul":0.5});
    assert!(request.decode_json(extra, "p", 0).is_err());
}
