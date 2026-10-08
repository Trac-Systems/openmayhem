use mayhem_proto::{
    endpoint_family_contract_template, metered_output_units, normalized_request_prompt_units,
    proxy::ProxyEndpoint, VisibleToolCall,
};
use mayhem_proxy::{
    endpoint::{Adapter, Limits},
    metering::{Disposition, Policy},
};
use serde_json::{json, Value};

fn adapter(endpoint: ProxyEndpoint) -> Adapter {
    let family = match endpoint {
        ProxyEndpoint::Chat => mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
        ProxyEndpoint::Completions => mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
        ProxyEndpoint::Responses => mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
        ProxyEndpoint::Decisions => mayhem_proto::ENDPOINT_MAYHEM_DECISIONS,
    };
    let mut contract = endpoint_family_contract_template(family).unwrap();
    // These profiles explicitly advertise the optional fields under test; native
    // templates are deliberately not broadened by the proxy implementation.
    let extra = match endpoint {
        ProxyEndpoint::Completions => vec![
            ("suffix", mayhem_proto::EndpointValueType::String),
            ("n", mayhem_proto::EndpointValueType::Integer),
        ],
        ProxyEndpoint::Responses => vec![("instructions", mayhem_proto::EndpointValueType::String)],
        _ => vec![],
    };
    for (key, kind) in extra {
        contract.request_attributes.push(key.into());
        contract
            .request_attribute_specs
            .insert(key.into(), mayhem_proto::EndpointAttributeSpec::new(kind));
    }
    if endpoint == ProxyEndpoint::Completions {
        contract
            .request_attribute_specs
            .get_mut("prompt")
            .unwrap()
            .value_types
            .push(mayhem_proto::EndpointValueType::Array);
    }
    Adapter::new(
        endpoint,
        contract,
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
    json!({"model":"public","messages":[{"role":"user","content":"hello"}]})
}
fn answer() -> Value {
    json!({"choices":[{"index":0,"message":{"role":"assistant","content":"答え🙂"},"finish_reason":"stop"}]})
}

#[test]
fn normalized_input_and_observed_output_ignore_vendor_usage_and_transport_settings() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let mut body = chat();
    body["messages"][0]["content"] = json!("你好, hello\n  🙂");
    let expected = normalized_request_prompt_units(&json!({"messages":body["messages"]})).unwrap();
    let mut observations = Vec::new();
    for (stream, claims) in [(false, 0), (true, 9_000_000)] {
        body["stream"] = json!(stream);
        body["temperature"] = json!(0.25);
        let bytes = serde_json::to_vec(&body).unwrap();
        let request = if stream {
            adapter.prepare_stream(&bytes)
        } else {
            adapter.prepare_json(&bytes)
        }
        .unwrap();
        let mut result = answer();
        result["usage"] = json!({"prompt_tokens":claims,"completion_tokens":claims,"total_tokens":claims*2,"prompt_tokens_details":{"cached_tokens":claims},"completion_tokens_details":{"reasoning_tokens":claims}});
        let reply = request.decode_json(result, "public_result", 123).unwrap();
        let observation = reply.observed_usage.unwrap();
        assert_eq!(observation.units["input_token"], expected);
        assert_eq!(observation.units["output_token"], 3); // 10 UTF-8 bytes / 4 rounded once.
        assert_eq!(observation.disposition, Disposition::Complete);
        assert!(reply.body.get("usage").is_none());
        observations.push(observation);
    }
    assert_eq!(observations[0].units, observations[1].units);
    assert_eq!(observations[0].result_hash, observations[1].result_hash);
    assert_ne!(observations[0].request_hash, observations[1].request_hash);
}

#[test]
fn tool_definitions_and_schema_count_as_input_tools_and_reasoning_as_output_once() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let mut body = chat();
    body["tools"] = json!([{"type":"function","function":{"name":"read","description":"read text","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}]);
    body["response_format"] =
        json!({"type":"json_schema","json_schema":{"name":"result","schema":{"type":"object"}}});
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let result = json!({"choices":[{"index":0,"message":{"role":"assistant","content":"","reasoning":"thinking","reasoning_content":"thinking","tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{\"path\":\"a.txt\"}"}}]},"finish_reason":"tool_calls"}]});
    let reply = request.decode_json(result.clone(), "r", 0).unwrap();
    let usage = reply.observed_usage.unwrap();
    let projection = json!({"messages":body["messages"],"tools":body["tools"],"response_format":body["response_format"]});
    assert_eq!(
        usage.units["input_token"],
        normalized_request_prompt_units(&projection).unwrap()
    );
    assert_eq!(
        usage.units["output_token"],
        metered_output_units(
            "",
            "thinking",
            &[VisibleToolCall {
                id: "c1".into(),
                name: "read".into(),
                arguments: "{\"path\":\"a.txt\"}".into()
            }]
        )
    );
    let mut ambiguous = result;
    ambiguous["choices"][0]["message"]["reasoning"] = json!("different");
    assert!(request.decode_json(ambiguous, "r", 0).is_err());
}

#[test]
fn canonical_input_has_no_key_order_whitespace_or_model_name_price_effect() {
    let p = Policy::ObservableTextV1;
    let a: Value = serde_json::from_str(
        r#"{"messages":[{"role":"user","content":"hi"}],"model":"x","temperature":1}"#,
    )
    .unwrap();
    let b:Value=serde_json::from_str(r#"{ "model":"a much longer alias", "messages": [ { "content":"hi", "role":"user" } ], "temperature":0 }"#).unwrap();
    let result = answer();
    let a = p
        .prepare(ProxyEndpoint::Chat, &a)
        .unwrap()
        .observe(&result)
        .unwrap();
    let b = p
        .prepare(ProxyEndpoint::Chat, &b)
        .unwrap()
        .observe(&result)
        .unwrap();
    assert_eq!(a.units, b.units);
    assert_ne!(a.request_hash, b.request_hash);
    assert!(Policy::resolve(ProxyEndpoint::Decisions, &p.hash()).is_err());
    assert_eq!(p.contract().units, vec!["input_token", "output_token"]);
}

#[test]
fn unsupported_media_opaque_input_and_token_id_prompts_cannot_be_metered_as_text() {
    let p = Policy::ObservableTextV1;
    for content in [
        json!([{"type":"image_url","image_url":{"url":"https://example.invalid/a"}}]),
        json!([{"type":"input_audio","input_audio":{"data":"abc","format":"wav"}}]),
        json!([{"type":"file","file":{"file_id":"abc"}}]),
        json!([{"type":"text","text":"hello","secret_media":"payload"}]),
    ] {
        let mut body = chat();
        body["messages"][0]["content"] = content;
        assert!(p.prepare(ProxyEndpoint::Chat, &body).is_err());
    }
    for input in [
        json!([{"type":"item_reference","id":"hidden-history"}]),
        json!([{"type":"reasoning","id":"r","encrypted_content":"opaque","summary":[]}]),
        json!([{"type":"function_call_output","call_id":"c","output":[{"type":"input_image","image_url":"x"}]}]),
    ] {
        assert!(p
            .prepare(
                ProxyEndpoint::Responses,
                &json!({"model":"m","input":input})
            )
            .is_err());
    }
    assert!(p
        .prepare(
            ProxyEndpoint::Completions,
            &json!({"model":"m","prompt":[1,2,3]})
        )
        .is_err());
    let mut body = chat();
    body["messages"][0]["content"] =
        json!([{"type":"text","text":"contains image_url as plain text"}]);
    assert!(p.prepare(ProxyEndpoint::Chat, &body).is_ok());
}

#[test]
fn completions_batch_counts_prompt_once_and_rounds_output_once_across_choices() {
    let adapter = adapter(ProxyEndpoint::Completions);
    let body = json!({"model":"m","prompt":["a","b"],"suffix":"end","n":2});
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let result = json!({"choices":(0..4).map(|i| json!({"index":i,"text":"x","finish_reason":"stop"})).collect::<Vec<_>>()});
    let usage = request
        .decode_json(result, "r", 0)
        .unwrap()
        .observed_usage
        .unwrap();
    assert_eq!(
        usage.units["input_token"],
        normalized_request_prompt_units(&json!({"prompt":["a","b"],"suffix":"end"})).unwrap()
    );
    assert_eq!(usage.units["output_token"], 1);
}

#[test]
fn responses_count_only_observable_content_not_ciphertext_citations_or_logprobs() {
    let adapter = adapter(ProxyEndpoint::Responses);
    let body = json!({"model":"m","input":"hello","instructions":"be brief"});
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let result = json!({"status":"completed","output":[{"id":"r1","type":"reasoning","content":[{"type":"reasoning_text","text":"abc"}],"summary":[{"type":"summary_text","text":"d"}],"encrypted_content":"invisible".repeat(1000)},{"id":"m1","type":"message","role":"assistant","content":[{"type":"output_text","text":"e","annotations":[{"type":"url_citation","url":"https://example.com","title":"not generated text","start_index":0,"end_index":1}],"logprobs":[{"token":"e","logprob":-1.}]}]}]});
    let reply = request.decode_json(result, "r", 0).unwrap();
    assert_eq!(reply.observed_usage.unwrap().units["output_token"], 2); // 5 visible bytes, not opaque metadata.
    assert_eq!(
        reply.body["output"][0]["encrypted_content"]
            .as_str()
            .unwrap()
            .len(),
        9000
    );
}

#[test]
fn refusals_and_incomplete_outputs_remain_observations_without_complete_charge_authority() {
    let adapter = adapter(ProxyEndpoint::Chat);
    let request = adapter
        .prepare_json(&serde_json::to_vec(&chat()).unwrap())
        .unwrap();
    for (finish, disposition) in [
        ("length", Disposition::Incomplete),
        ("content_filter", Disposition::Refused),
    ] {
        let mut output = answer();
        output["choices"][0]["finish_reason"] = json!(finish);
        assert_eq!(
            request
                .decode_json(output, "r", 0)
                .unwrap()
                .observed_usage
                .unwrap()
                .disposition,
            disposition
        );
    }
    let mut output = answer();
    output["choices"][0]["message"]["refusal"] = json!("no");
    assert_eq!(
        request
            .decode_json(output, "r", 0)
            .unwrap()
            .observed_usage
            .unwrap()
            .disposition,
        Disposition::Refused
    );
}

#[test]
fn decisions_bill_question_units_not_generated_tokens_probabilities_or_input_bytes() {
    let adapter = adapter(ProxyEndpoint::Decisions);
    let body = json!({"model":"m","state":"context".repeat(200),"questions":{"q":{"type":"choice","instructions":"choose","criteria":{"a":"A","b":"B"}},"s":{"type":"score","instructions":"score","criteria":["low","high"]},"n":{"type":"noul","instructions":"noul?"}}});
    let request = adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let output = json!({"answers":{"q":{"type":"choice","choice":"a","probabilities":{"a":0.6,"b":0.4}},"s":{"type":"score","score":0.7,"probabilities":{"0":0.3,"1":0.7}},"n":{"type":"noul","noul":0.9}},"usage":{"input_tokens":1000000,"output_tokens":1000000}});
    let reply = request.decode_json(output.clone(), "r", 0).unwrap();
    assert_eq!(
        reply.observed_usage.unwrap().units,
        std::collections::BTreeMap::from([("decision".into(), 3)])
    );
    let mut invalid = output;
    invalid["answers"].as_object_mut().unwrap().remove("q");
    assert!(request.decode_json(invalid, "r", 0).is_err());
}
