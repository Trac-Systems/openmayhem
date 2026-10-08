use mayhem_proto::{endpoint_family_contract_template, proxy::ProxyEndpoint};
use mayhem_proxy::{
    endpoint::{stream::Stream, Adapter, Limits},
    worker::Decoded,
};
use serde_json::{json, Value};

fn request() -> mayhem_proxy::endpoint::Request {
    let adapter = Adapter::new(
        ProxyEndpoint::Chat,
        endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS).unwrap(),
        "backend".into(),
        Limits {
            request_bytes: 1024 * 1024,
            response_bytes: 4 * 1024 * 1024,
            choices: 8,
            tools: 32,
            questions: 16,
            decision_options: 32,
        },
    )
    .unwrap();
    adapter
        .prepare_stream(
            br#"{"model":"m","messages":[{"role":"user","content":"test"}],"stream":true}"#,
        )
        .unwrap()
}
fn frame(value: Value) -> Decoded {
    Decoded::Sse {
        event: String::new(),
        data: value.to_string(),
        id: None,
    }
}
fn done() -> Decoded {
    Decoded::Sse {
        event: String::new(),
        data: "[DONE]".into(),
        id: None,
    }
}
fn chunk(content: &str, finish: Value) -> Decoded {
    frame(
        json!({"id":"id","choices":[{"index":0,"delta":{"content":content},"finish_reason":finish}]}),
    )
}

#[test]
fn missing_finish_data_after_finish_or_done_and_duplicate_choices_poison_stream() {
    for mode in 0..4 {
        let request = request();
        let mut stream = Stream::new(&request, "public", 0).unwrap();
        stream
            .push(chunk(
                "text",
                if mode == 0 {
                    Value::Null
                } else {
                    json!("stop")
                },
            ))
            .unwrap();
        let failure=match mode {
            0=>stream.push(done()),
            1=>stream.push(chunk("illegal extra text",Value::Null)),
            2=>{stream.push(done()).unwrap();stream.push(chunk("after done",Value::Null))},
            _=>stream.push(frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"},{"index":0,"delta":{},"finish_reason":"stop"}]}))),
        };
        assert!(failure.is_err());
        assert!(stream.finish().is_err());
    }
}

#[test]
fn usage_in_final_choice_chunk_is_accepted_and_claim_is_not_emitted_to_buyer() {
    let request = request();
    let mut stream = Stream::new(&request, "public", 0).unwrap();
    let event = frame(
        json!({"id":"id","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}),
    );
    let delta = stream.push(event).unwrap().unwrap();
    assert!(delta.get("usage").is_none());
    assert!(delta["choices"][0]["finish_reason"].is_null());
    stream.push(done()).unwrap();
    let value = stream.finish().unwrap();
    assert_eq!(value["usage"]["total_tokens"], 2);
}

#[test]
fn many_chunks_retain_final_content_without_a_chunk_history_or_synthetic_terminal() {
    let request = request();
    let mut stream = Stream::new(&request, "public", 0).unwrap();
    for _ in 0..10_000 {
        assert!(stream.push(chunk("x", Value::Null)).unwrap().is_some());
    }
    stream.push(chunk("", json!("stop"))).unwrap();
    stream.push(done()).unwrap();
    let value = stream.finish().unwrap();
    assert_eq!(
        value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .len(),
        10_000
    );
    let mut unfinished = Stream::new(&request, "public", 0).unwrap();
    unfinished.push(chunk("text", json!("stop"))).unwrap();
    assert!(unfinished.finish().is_err());
}
