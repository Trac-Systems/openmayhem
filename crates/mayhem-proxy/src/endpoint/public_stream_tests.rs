use super::*;
use crate::worker::Decoded;
#[path = "../../tests/support/responses.rs"]
mod responses;

fn fixture(endpoint: ProxyEndpoint) -> (PublicRequest, Vec<Value>, Value) {
    let (contract, body, raw) = match endpoint {
        ProxyEndpoint::Chat => (
            mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            json!({"model":"public","messages":[{"role":"user","content":"hi"}],"stream":true}),
            vec![
                json!({"id":"upstream","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}),
                json!({"id":"upstream","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":"stop"}]}),
            ],
        ),
        ProxyEndpoint::Completions => (
            mayhem_proto::ENDPOINT_OPENAI_COMPLETIONS,
            json!({"model":"public","prompt":"hi","stream":true}),
            vec![
                json!({"id":"upstream","choices":[{"index":0,"text":"hello","finish_reason":null}]}),
                json!({"id":"upstream","choices":[{"index":0,"text":" world","finish_reason":"stop"}]}),
            ],
        ),
        ProxyEndpoint::Responses => (
            mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
            serde_json::from_slice(&responses::body()).unwrap(),
            responses::flow(r#"{"city":"Paris"}"#),
        ),
        _ => unreachable!(),
    };
    let adapter = Adapter::new(
        endpoint,
        mayhem_proto::endpoint_family_contract_template(contract).unwrap(),
        "upstream".into(),
        Limits {
            request_bytes: 64 * 1024,
            response_bytes: 64 * 1024,
            choices: 8,
            tools: 8,
            questions: 8,
            decision_options: 8,
        },
    )
    .unwrap();
    let bytes = serde_json::to_vec(&body).unwrap();
    let provider = adapter.prepare_stream(&bytes).unwrap();
    let mut stream = stream::Stream::new(&provider, "proxy_fixture", 7).unwrap();
    let mut events = vec![];
    for value in raw {
        if let Some(event) = stream
            .push(Decoded::Sse {
                event: String::new(),
                data: value.to_string(),
                id: None,
            })
            .unwrap()
        {
            events.push(event);
        }
    }
    if endpoint != ProxyEndpoint::Responses {
        stream
            .push(Decoded::Sse {
                event: String::new(),
                data: "[DONE]".into(),
                id: None,
            })
            .unwrap();
    }
    let result = provider
        .decode_json(stream.finish().unwrap(), "proxy_fixture", 7)
        .unwrap()
        .body;
    let buyer = PublicAdapter::restore(adapter.public_snapshot())
        .unwrap()
        .prepare_stream(&bytes)
        .unwrap();
    (buyer, events, result)
}

#[test]
fn normalized_stream_agrees_with_real_provider_text_tools_reasoning_and_final() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
    ] {
        let (request, events, result) = fixture(endpoint);
        let mut stream = request.stream("proxy_fixture", 7).unwrap();
        for event in &events {
            stream.push(event).unwrap();
        }
        stream.verify_final(&result).unwrap();
    }
}
#[test]
fn normalized_stream_rejects_changed_id_fields_sequences_and_final_content() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
    ] {
        let (request, events, result) = fixture(endpoint);
        for change in ["identity", "field", "terminal"] {
            let mut event = events[0].clone();
            match change {
                "identity" if endpoint == ProxyEndpoint::Responses => {
                    event["response"]["id"] = json!("different")
                }
                "identity" => event["id"] = json!("different"),
                "field" => event["unexpected"] = json!(true),
                _ if endpoint == ProxyEndpoint::Responses => {
                    event["type"] = json!("response.completed")
                }
                _ => event["choices"][0]["finish_reason"] = json!("stop"),
            }
            let mut stream = request.stream("proxy_fixture", 7).unwrap();
            assert!(stream.push(&event).is_err(), "{endpoint:?}/{change}");
            assert!(stream.verify_final(&result).is_err());
        }
        let mut stream = request.stream("proxy_fixture", 7).unwrap();
        for event in &events {
            stream.push(event).unwrap();
        }
        let mut changed = result.clone();
        match endpoint {
            ProxyEndpoint::Chat => changed["choices"][0]["message"]["content"] = json!("other"),
            ProxyEndpoint::Completions => changed["choices"][0]["text"] = json!("other"),
            ProxyEndpoint::Responses => changed["output"][1]["content"][0]["text"] = json!("other"),
            _ => unreachable!(),
        }
        assert!(stream.verify_final(&changed).is_err());
        if endpoint == ProxyEndpoint::Responses {
            let mut stream = request.stream("proxy_fixture", 7).unwrap();
            let mut event = events[0].clone();
            event["sequence_number"] = json!(1);
            assert!(stream.push(&event).is_err());
        }
    }
}

#[test]
fn normalized_stream_public_verifier_bounds_input_before_encoding_or_cloning() {
    let (request, events, result) = fixture(ProxyEndpoint::Chat);
    let mut stream = request.stream("proxy_fixture", 7).unwrap();
    let mut huge = events[0].clone();
    huge["choices"][0]["delta"]["content"] = json!("x".repeat(64 * 1024));
    assert!(stream.push(&huge).is_err());
    assert!(stream.verify_final(&result).is_err());
    let mut stream = request.stream("proxy_fixture", 7).unwrap();
    for event in &events {
        stream.push(event).unwrap();
    }
    let mut huge = result;
    huge["choices"][0]["message"]["content"] = json!("x".repeat(64 * 1024));
    assert!(stream.verify_final(&huge).is_err());
}
