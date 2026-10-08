use mayhem_proto::{endpoint_family_contract_template, proxy::ProxyEndpoint};
use mayhem_proxy::{
    endpoint::{stream::Stream, Adapter, Limits},
    worker::Decoded,
};
use serde_json::{json, Value};
#[path = "support/responses.rs"]
mod fixture;

fn request() -> mayhem_proxy::endpoint::Request {
    Adapter::new(
        ProxyEndpoint::Responses,
        endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_RESPONSES).unwrap(),
        "upstream-model".into(),
        Limits {
            request_bytes: 1024 * 1024,
            response_bytes: 4 * 1024 * 1024,
            choices: 8,
            tools: 32,
            questions: 16,
            decision_options: 32,
        },
    )
    .unwrap()
    .prepare_stream(&fixture::body())
    .unwrap()
}
fn frame(event: &Value) -> Decoded {
    Decoded::Sse {
        event: event["type"].as_str().unwrap().into(),
        data: event.to_string(),
        id: None,
    }
}
fn run(events: &[Value]) -> Result<Value, mayhem_proxy::endpoint::Error> {
    let request = request();
    let mut s = Stream::new(&request, "public", 10)?;
    for e in events {
        s.push(frame(e))?;
    }
    s.finish()
}
#[test]
fn response_stream_keeps_reasoning_tools_unicode_and_citations_without_private_metadata() {
    let request = request();
    let mut s = Stream::new(&request, "public", 10).unwrap();
    let mut received = Vec::new();
    for e in fixture::flow(r#"{"city":"München"}"#) {
        if let Some(delta) = s.push(frame(&e)).unwrap() {
            received.push(delta);
        }
    }
    assert!(s.is_done());
    let raw = s.finish().unwrap();
    let reply = request.decode_json(raw, "public", 10).unwrap();
    assert_eq!(
        reply.body["output"][0]["summary"][0]["text"],
        "Checking weather"
    );
    assert_eq!(
        reply.body["output"][0]["content"][0]["text"],
        "Use the declared function."
    );
    assert_eq!(
        reply.body["output"][0]["encrypted_content"],
        "opaque-continuation"
    );
    assert_eq!(
        reply.body["output"][1]["content"][0]["text"],
        "Checking München 🌧."
    );
    assert_eq!(
        reply.body["output"][1]["content"][0]["annotations"][0]["url"],
        "https://example.org/weather"
    );
    assert_eq!(
        reply.body["output"][2]["arguments"],
        r#"{"city":"München"}"#
    );
    assert_eq!(reply.reported_usage.unwrap().reasoning_tokens, Some(4));
    for (i, e) in received.iter().enumerate() {
        assert_eq!(e["sequence_number"], i);
        assert!(!e["type"].as_str().unwrap().ends_with(".done"));
        assert_ne!(e["type"], "response.completed");
        assert!(!e.to_string().contains("private-upstream-settings"));
        assert!(!e.to_string().contains("private-backend-model"));
    }
}
#[test]
fn malformed_event_sequences_cannot_be_replaced_by_a_valid_final_snapshot() {
    for case in 0..13 {
        let mut events = fixture::flow(r#"{"city":"München"}"#);
        match case {
            0 => {
                events.pop();
            }
            1 => {
                let n = events.len() - 1;
                events[n]["response"]["output"][2]["arguments"] = json!(r#"{"city":"Berlin"}"#);
            }
            2 => {
                let n = events.len() - 1;
                events[n]["response"]["id"] = json!("spliced");
            }
            3 => events[1]["sequence_number"] = json!(0),
            4 => {
                let e = events
                    .iter_mut()
                    .find(|e| e["type"] == "response.output_text.delta")
                    .unwrap();
                e["item_id"] = json!("wrong-item");
            }
            5 => {
                let e = events
                    .iter_mut()
                    .find(|e| e["type"] == "response.function_call_arguments.done")
                    .unwrap();
                e["arguments"] = json!(r#"{"city":"Berlin"}"#);
            }
            6 => {
                let e = events
                    .iter_mut()
                    .find(|e| e["type"] == "response.output_text.done")
                    .unwrap();
                e["text"] = json!("changed text");
            }
            7 => {
                let e = events
                    .iter_mut()
                    .find(|e| e["type"] == "response.output_item.added")
                    .unwrap();
                e["output_index"] = json!(1);
            }
            8 => {
                events.retain(|e| e["type"] != "response.function_call_arguments.done");
            }
            9 => {
                events.retain(|e| e["type"] != "response.output_item.done");
            }
            10 => {
                let e = events
                    .iter_mut()
                    .find(|e| {
                        e["type"] == "response.output_item.added"
                            && e["item"]["type"] == "function_call"
                    })
                    .unwrap();
                e["item"]["name"] = json!("undeclared");
            }
            11 => {
                let e = events.last().unwrap().clone();
                events.push(e);
            }
            _ => {
                let e = events
                    .iter_mut()
                    .find(|e| e["type"] == "response.output_text.annotation.added")
                    .unwrap();
                e["annotation"]["url"] = json!("javascript:alert(1)");
            }
        }
        assert!(run(&events).is_err(), "case {case}");
    }
}
#[test]
fn incomplete_partial_function_arguments_are_data_not_executable_tools() {
    let mut events = Vec::new();
    fixture::append(
        &mut events,
        "response.created",
        json!({"response":fixture::response("in_progress",json!([]))}),
    );
    let mut item = json!({"id":"f","type":"function_call","call_id":"c","name":"lookup","arguments":"","status":"in_progress"});
    fixture::append(
        &mut events,
        "response.output_item.added",
        json!({"output_index":0,"item":item}),
    );
    fixture::append(
        &mut events,
        "response.function_call_arguments.delta",
        json!({"output_index":0,"item_id":"f","delta":"{\"city\":"}),
    );
    item["arguments"] = json!("{\"city\":");
    item["status"] = json!("incomplete");
    let mut response = fixture::response("incomplete", json!([item]));
    response["incomplete_details"] = json!({"reason":"max_output_tokens"});
    fixture::append(
        &mut events,
        "response.incomplete",
        json!({"response":response}),
    );
    let result = run(&events).unwrap();
    assert_eq!(result["status"], "incomplete");
    assert_eq!(result["output"][0]["status"], "incomplete");
    assert_eq!(result["output"][0]["arguments"], "{\"city\":");
}
#[test]
fn failure_and_sse_type_mismatch_poison_the_stream() {
    let request = request();
    let mut s = Stream::new(&request, "public", 10).unwrap();
    let mut f = frame(&fixture::flow("{}")[0]);
    if let Decoded::Sse { event, .. } = &mut f {
        *event = "response.completed".into();
    }
    assert!(s.push(f).is_err());
    assert!(s.finish().is_err());
    let mut events = fixture::flow("{}");
    events.pop();
    fixture::append(
        &mut events,
        "response.failed",
        json!({"response":{"id":"upstream-response","status":"failed","error":{"code":"server_error","message":"private-info"}}}),
    );
    assert!(run(&events).is_err());
}

#[test]
fn interleaved_items_and_parallel_calls_preserve_identity_and_declared_parallel_limit() {
    let events = fixture::flow(r#"{"city":"München"}"#);
    // All items start first, then reasoning/text/arguments progress independently.
    let mut reordered = Vec::new();
    for e in events
        .iter()
        .filter(|e| {
            matches!(
                e["type"].as_str(),
                Some("response.created" | "response.in_progress" | "response.output_item.added")
            )
        })
        .chain(events.iter().filter(|e| {
            !matches!(
                e["type"].as_str(),
                Some("response.created" | "response.in_progress" | "response.output_item.added")
            )
        }))
    {
        let mut e = e.clone();
        e["sequence_number"] = json!(reordered.len());
        reordered.push(e);
    }
    assert_eq!(
        run(&reordered).unwrap()["output"].as_array().unwrap().len(),
        3
    );
    let mut events = Vec::new();
    fixture::append(
        &mut events,
        "response.created",
        json!({"response":fixture::response("in_progress",json!([]))}),
    );
    let mut items = vec![];
    for i in 0..2 {
        let item = json!({"id":format!("f{i}"),"type":"function_call","call_id":format!("c{i}"),"name":"lookup","arguments":"","status":"in_progress"});
        fixture::append(
            &mut events,
            "response.output_item.added",
            json!({"output_index":i,"item":item}),
        );
        items.push(item);
    }
    for (i, item) in items.iter_mut().enumerate() {
        let args = if i == 0 {
            r#"{"city":"Paris"}"#
        } else {
            r#"{"city":"Berlin"}"#
        };
        fixture::append(
            &mut events,
            "response.function_call_arguments.delta",
            json!({"output_index":i,"item_id":item["id"],"delta":args}),
        );
        fixture::append(
            &mut events,
            "response.function_call_arguments.done",
            json!({"output_index":i,"item_id":item["id"],"arguments":args}),
        );
        item["arguments"] = json!(args);
        item["status"] = json!("completed");
        fixture::append(
            &mut events,
            "response.output_item.done",
            json!({"output_index":i,"item":item}),
        );
    }
    fixture::append(
        &mut events,
        "response.completed",
        json!({"response":fixture::response("completed",json!(items))}),
    );
    assert_eq!(run(&events).unwrap()["output"].as_array().unwrap().len(), 2);
    let adapter = Adapter::new(
        ProxyEndpoint::Responses,
        endpoint_family_contract_template(mayhem_proto::ENDPOINT_OPENAI_RESPONSES).unwrap(),
        "upstream-model".into(),
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
    let mut body: Value = serde_json::from_slice(&fixture::body()).unwrap();
    body["parallel_tool_calls"] = json!(false);
    let request = adapter
        .prepare_stream(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    let mut stream = Stream::new(&request, "public", 0).unwrap();
    stream.push(frame(&events[0])).unwrap();
    stream.push(frame(&events[1])).unwrap();
    assert!(stream.push(frame(&events[2])).is_err());
}

#[test]
fn refusals_logprobs_and_long_fragmented_text_keep_their_meaning() {
    let refusal = run(&fixture::text_flow("I cannot fulfil that request.", true)).unwrap();
    assert_eq!(refusal["output"][0]["content"][0]["type"], "refusal");
    let mut events = fixture::text_flow("A", false);
    let logs = json!([{"token":"A","logprob":-0.1,"top_logprobs":[{"token":"B","logprob":-2.0}]}]);
    for e in &mut events {
        match e["type"].as_str().unwrap() {
            "response.output_text.delta" | "response.output_text.done" => {
                e["logprobs"] = logs.clone()
            }
            "response.content_part.done" => e["part"]["logprobs"] = logs.clone(),
            "response.output_item.done" => e["item"]["content"][0]["logprobs"] = logs.clone(),
            "response.completed" => {
                e["response"]["output"][0]["content"][0]["logprobs"] = logs.clone()
            }
            _ => (),
        }
    }
    assert_eq!(
        run(&events).unwrap()["output"][0]["content"][0]["logprobs"],
        logs
    );
    let text = "x".repeat(10_000);
    assert_eq!(
        run(&fixture::text_flow(&text, false)).unwrap()["output"][0]["content"][0]["text"],
        text
    );
}
