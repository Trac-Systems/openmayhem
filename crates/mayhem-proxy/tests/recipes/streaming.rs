use super::*;
fn field(value: Value) -> Value {
    json!({"optional":false,"value":value})
}
fn optional(path: &[&str]) -> Value {
    json!({"optional":true,"value":{"kind":"copy","path":path}})
}
fn literal(value: Value) -> Value {
    json!({"kind":"literal","value":value})
}
fn copy(path: &[&str]) -> Value {
    json!({"kind":"copy","path":path})
}
fn normalized(value: Value) -> Value {
    json!({"kind":"sse","event":"message","data":String::from_utf8(mayhem_proto::stable_json_bytes(&value).unwrap()).unwrap(),"id":null})
}
fn wrapper(value: Value, ndjson: bool) -> Value {
    json!({"event":if ndjson{Value::Null}else{json!("message")},"data":value,"id":null})
}
fn stream_recipe(ndjson: bool) -> mayhem_proxy::recipe::Signed {
    let mut recipe = recipe_fixture::recipe(ProxyEndpoint::Chat).recipe;
    let delta = json!({"kind":"object","fields":{"id":field(copy(&["data","job"])),"choices":field(json!({"kind":"tuple","items":[{"kind":"object","fields":{"index":field(literal(json!(0))),"delta":field(json!({"kind":"object","fields":{"role":field(literal(json!("assistant"))),"content":optional(&["data","text"]),"tool_calls":optional(&["data","calls"])}}))}}]}))}});
    let terminal = json!({"kind":"object","fields":{"id":field(copy(&["data","job"])),"choices":field(json!({"kind":"tuple","items":[{"kind":"object","fields":{"index":field(literal(json!(0))),"delta":field(json!({"kind":"object","fields":{}})),"finish_reason":field(json!({"kind":"enum","path":["data","reason"],"values":{"end":"stop","tools":"tool_calls","limit":"length"}}))}}]}))}});
    recipe.abi_min = 2;
    recipe.abi_max = 2;
    recipe.stream=Some(serde_json::from_value(json!({"format":if ndjson{"ndjson"}else{"sse"},"event_path":["data","tag"],"max_events":32,
        "events":{"delta":{"kind":"data","event":"message","terminal":false,"value":delta},"terminal":{"kind":"data","event":"message","terminal":false,"value":terminal},"done":{"kind":"finish"},"busy":{"kind":"error","error":"busy"},"heartbeat":{"kind":"heartbeat"}},
        "fixtures":[{"input":[wrapper(json!({"tag":"delta","job":"j1","text":"Hi"}),ndjson),wrapper(json!({"tag":"terminal","job":"j1","reason":"end"}),ndjson),wrapper(json!({"tag":"done"}),ndjson)],"output":[normalized(json!({"id":"j1","choices":[{"index":0,"delta":{"role":"assistant","content":"Hi"}}]})),normalized(json!({"id":"j1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})),{"kind":"sse","event":"message","data":"[DONE]","id":null}]}]})).unwrap());
    recipe_fixture::sign(recipe)
}
fn wire(events: &[Value], ndjson: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for event in events {
        if !ndjson {
            bytes.extend_from_slice(b"data: ");
        }
        bytes.extend(serde_json::to_vec(event).unwrap());
        bytes.extend_from_slice(if ndjson { b"\n" } else { b"\n\n" });
    }
    bytes
}
async fn run_stream(
    events: Vec<Value>,
    ndjson: bool,
    tool: bool,
) -> (
    std::result::Result<mayhem_proxy::execution::UnsettledReply, Error>,
    usize,
    Vec<Value>,
) {
    let recipe = stream_recipe(ndjson);
    let mut body = if tool {
        tool_request(&recipe)
    } else {
        recipe.recipe.fixtures[0].request.clone()
    };
    body["stream"] = json!(true);
    let server = backend_raw(
        200,
        wire(&events, ndjson),
        if ndjson {
            "application/x-ndjson"
        } else {
            "text/event-stream"
        },
        Duration::ZERO,
    )
    .await;
    let fixture = custom(&server.base, recipe);
    let bytes = serde_json::to_vec(&body).unwrap();
    let record = fixture.prepare_kind(920, &bytes, ProxyRail::Fiat, true);
    let mut output = Vec::new();
    let result = fixture
        .executor
        .execute_stream(&record.invocation, &bytes, &Cancellation::default(), |v| {
            output.push(v);
            async { Ok(()) }
        })
        .await;
    if result.is_ok() {
        assert!(fixture
            .journal
            .recover(&record.invocation, record.attempt)
            .unwrap()
            .result
            .is_some());
    }
    (result, server.calls.load(Ordering::SeqCst), output)
}
#[tokio::test]
async fn custom_sse_and_ndjson_streams_preserve_utf8_order_final_verification_and_metering() {
    for ndjson in [false, true] {
        let (result, calls, output) = run_stream(
            vec![
                json!({"tag":"heartbeat"}),
                json!({"tag":"delta","job":"j1","text":"Hello λ"}),
                json!({"tag":"delta","job":"j1","text":" 🌍"}),
                json!({"tag":"terminal","job":"j1","reason":"end"}),
                json!({"tag":"done"}),
            ],
            ndjson,
            false,
        )
        .await;
        let result = result.unwrap();
        assert_eq!(
            result.reply.body["choices"][0]["message"]["content"],
            "Hello λ 🌍"
        );
        assert_eq!(calls, 1);
        assert!(!output.is_empty());
        assert!(result.reply.observed_usage.is_some());
    }
}
#[tokio::test]
async fn custom_streams_reject_missing_unknown_or_duplicate_terminals_and_remote_errors() {
    for ndjson in [false, true] {
        for tail in [
            vec![],
            vec![json!({"tag":"unknown"})],
            vec![json!({"tag":"done"})],
            vec![json!({"tag":"busy"})],
            vec![
                json!({"tag":"terminal","job":"j1","reason":"end"}),
                json!({"tag":"terminal","job":"j1","reason":"end"}),
                json!({"tag":"done"}),
            ],
        ] {
            let mut events = vec![json!({"tag":"delta","job":"j1","text":"Hi"})];
            events.extend(tail);
            let (result, calls, _) = run_stream(events, ndjson, false).await;
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }
}
#[tokio::test]
async fn custom_stream_tool_fragments_are_assembled_then_checked_against_original_schema() {
    for ndjson in [false, true] {
        for (end, valid) in [("\"abc\" }", true), ("1 }", false)] {
            let (result,_,_)=run_stream(vec![json!({"tag":"delta","job":"j1","calls":[{"index":0,"id":"c1","type":"function","function":{"name":"lookup","arguments":"{ \"key\": "}}]}),json!({"tag":"delta","job":"j1","calls":[{"index":0,"function":{"arguments":end}}]}),json!({"tag":"terminal","job":"j1","reason":"tools"}),json!({"tag":"done"})],ndjson,true).await;
            assert_eq!(result.is_ok(), valid);
            if let Ok(result) = result {
                assert_eq!(
                    result.reply.body["choices"][0]["message"]["tool_calls"][0]["function"]
                        ["arguments"],
                    "{ \"key\": \"abc\" }"
                );
            }
        }
    }
}

#[tokio::test]
async fn custom_streams_reuse_completions_and_responses_terminal_consistency() {
    for endpoint in [ProxyEndpoint::Completions, ProxyEndpoint::Responses] {
        for ndjson in [false, true] {
            let mut recipe = recipe_fixture::recipe(endpoint).recipe;
            recipe.abi_min = 2;
            recipe.abi_max = 2;
            let common = if endpoint == ProxyEndpoint::Responses {
                response_fixture::text_flow("Hi λ", false)
            } else {
                vec![
                    json!({"id":"j1","choices":[{"index":0,"text":"Hi λ","finish_reason":null}]}),
                    json!({"id":"j1","choices":[{"index":0,"text":"","finish_reason":"stop"}]}),
                ]
            };
            let mut events = serde_json::Map::new();
            let mut input = Vec::new();
            let mut output = Vec::new();
            let mut raw = Vec::new();
            for value in common {
                let name = if endpoint == ProxyEndpoint::Responses {
                    value["type"].as_str().unwrap()
                } else {
                    "message"
                };
                let terminal = matches!(name, "response.completed" | "response.incomplete");
                events.insert(name.into(),json!({"kind":"data","event":name,"terminal":terminal,"value":copy(&["data","payload"])}));
                let wrapped = json!({"tag":name,"payload":value});
                input.push(wrapper(wrapped.clone(), ndjson));
                raw.push(wrapped);
                output.push(json!({"kind":"sse","event":name,"data":serde_json::to_string(&value).unwrap(),"id":null}));
            }
            if endpoint != ProxyEndpoint::Responses {
                events.insert("done".into(), json!({"kind":"finish"}));
                raw.push(json!({"tag":"done"}));
                input.push(wrapper(json!({"tag":"done"}), ndjson));
                output.push(json!({"kind":"sse","event":"message","data":"[DONE]","id":null}));
            }
            recipe.stream=Some(serde_json::from_value(json!({"format":if ndjson{"ndjson"}else{"sse"},"event_path":["data","tag"],"events":events,"max_events":64,"fixtures":[{"input":input,"output":output}]})).unwrap());
            let mut body = recipe.fixtures[0].request.clone();
            body["stream"] = json!(true);
            let server = backend_raw(
                200,
                wire(&raw, ndjson),
                if ndjson {
                    "application/x-ndjson"
                } else {
                    "text/event-stream"
                },
                Duration::ZERO,
            )
            .await;
            let fixture = custom(&server.base, recipe_fixture::sign(recipe));
            let bytes = serde_json::to_vec(&body).unwrap();
            let record = fixture.prepare_kind(922, &bytes, ProxyRail::Fiat, true);
            let result = fixture
                .executor
                .execute_stream(
                    &record.invocation,
                    &bytes,
                    &Cancellation::default(),
                    |_| async { Ok(()) },
                )
                .await
                .unwrap();
            assert!(serde_json::to_string(&result.reply.body)
                .unwrap()
                .contains("Hi λ"));
            assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn custom_stream_capability_bounds_and_offline_exports_are_explicit() {
    for ndjson in [false, true] {
        let signed = stream_recipe(ndjson);
        let raw = serde_json::to_value(&signed.recipe).unwrap();
        for (pointer, value) in [
            ("/abi_min", json!(1)),
            ("/stream/max_events", json!(0)),
            ("/stream/max_events", json!(65_537)),
            ("/stream/event_path", json!([])),
            ("/stream/events", json!({})),
        ] {
            let mut bad = raw.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            let recipe: mayhem_proxy::recipe::Recipe = serde_json::from_value(bad).unwrap();
            assert!(recipe.signing_bytes().is_err());
        }
        if let Ok(path) = std::env::var("PROXY_RECIPE_V2_FIXTURE_DIR") {
            std::fs::write(
                std::path::Path::new(&path).join(if ndjson {
                    "Chat-ndjson.json"
                } else {
                    "Chat-sse.json"
                }),
                signed.export().unwrap(),
            )
            .unwrap();
        }
    }
}
