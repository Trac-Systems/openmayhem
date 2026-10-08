use serde_json::{json, Value};

pub fn body() -> Vec<u8> {
    serde_json::to_vec(&json!({"model":"public-model","input":"Look up the weather","stream":true,
        "tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"],"additionalProperties":false}}]})).unwrap()
}
pub fn response(status: &str, output: Value) -> Value {
    json!({"id":"upstream-response","object":"response","status":status,"output":output,
        "model":"private-backend-model","instructions":"private-upstream-settings","error":null})
}
pub fn append(events: &mut Vec<Value>, kind: &str, mut fields: Value) {
    fields["type"] = json!(kind);
    fields["sequence_number"] = json!(events.len());
    events.push(fields);
}
pub fn text_flow(text: &str, refusal: bool) -> Vec<Value> {
    let mut events = Vec::new();
    append(
        &mut events,
        "response.created",
        json!({"response":response("in_progress",json!([]))}),
    );
    let mut item =
        json!({"id":"m","type":"message","role":"assistant","status":"in_progress","content":[]});
    append(
        &mut events,
        "response.output_item.added",
        json!({"output_index":0,"item":item}),
    );
    let key = if refusal { "refusal" } else { "text" };
    let kind = if refusal { "refusal" } else { "output_text" };
    let mut part = json!({"type":kind,key:""});
    if !refusal {
        part["annotations"] = json!([]);
        part["logprobs"] = json!([]);
    }
    append(
        &mut events,
        "response.content_part.added",
        json!({"output_index":0,"item_id":"m","content_index":0,"part":part}),
    );
    for c in text.chars() {
        append(
            &mut events,
            &format!("response.{kind}.delta"),
            json!({"output_index":0,"item_id":"m","content_index":0,"delta":c.to_string()}),
        );
    }
    append(
        &mut events,
        &format!("response.{kind}.done"),
        json!({"output_index":0,"item_id":"m","content_index":0,key:text}),
    );
    part[key] = json!(text);
    append(
        &mut events,
        "response.content_part.done",
        json!({"output_index":0,"item_id":"m","content_index":0,"part":part}),
    );
    item["content"] = json!([part]);
    item["status"] = json!("completed");
    append(
        &mut events,
        "response.output_item.done",
        json!({"output_index":0,"item":item}),
    );
    append(
        &mut events,
        "response.completed",
        json!({"response":response("completed",json!([item]))}),
    );
    events
}
pub fn flow(arguments: &str) -> Vec<Value> {
    let mut events = Vec::new();
    append(
        &mut events,
        "response.created",
        json!({"response":response("in_progress",json!([]))}),
    );
    append(
        &mut events,
        "response.in_progress",
        json!({"response":response("in_progress",json!([]))}),
    );
    let mut items = vec![
        json!({"id":"rs_1","type":"reasoning","summary":[],"content":[]}),
        json!({"id":"msg_1","type":"message","role":"assistant","status":"in_progress","content":[]}),
        json!({"id":"fn_1","type":"function_call","call_id":"call_1","name":"lookup","arguments":"","status":"in_progress"}),
    ];
    for i in 0..items.len() {
        append(
            &mut events,
            "response.output_item.added",
            json!({"output_index":i,"item":items[i]}),
        );
        if i == 2 {
            let halfway = arguments
                .char_indices()
                .nth(arguments.chars().count() / 2)
                .map(|(i, _)| i)
                .unwrap_or(0);
            for chunk in [&arguments[..halfway], &arguments[halfway..]] {
                append(
                    &mut events,
                    "response.function_call_arguments.delta",
                    json!({"output_index":i,"item_id":items[i]["id"],"delta":chunk}),
                );
            }
            items[i]["arguments"] = json!(arguments);
            append(
                &mut events,
                "response.function_call_arguments.done",
                json!({"output_index":i,"item_id":items[i]["id"],"arguments":arguments}),
            );
        } else {
            let parts: Vec<(bool, &str, &str)> = if i == 0 {
                vec![
                    (true, "summary_text", "Checking weather"),
                    (false, "reasoning_text", "Use the declared function."),
                ]
            } else {
                vec![(false, "output_text", "Checking München 🌧.")]
            };
            for (summary, kind, text) in parts {
                let field = if summary { "summary" } else { "content" };
                let index_field = if summary {
                    "summary_index"
                } else {
                    "content_index"
                };
                let part_events = if summary {
                    "response.reasoning_summary_part"
                } else {
                    "response.content_part"
                };
                let text_events = if summary {
                    "response.reasoning_summary_text"
                } else if i == 0 {
                    "response.reasoning_text"
                } else {
                    "response.output_text"
                };
                let mut part = json!({"type":kind,"text":""});
                if i == 1 {
                    part["annotations"] = json!([]);
                    part["logprobs"] = json!([]);
                }
                let fields =
                    json!({"output_index":i,"item_id":items[i]["id"],index_field:0,"part":part});
                append(&mut events, &format!("{part_events}.added"), fields);
                append(
                    &mut events,
                    &format!("{text_events}.delta"),
                    json!({"output_index":i,"item_id":items[i]["id"],index_field:0,"delta":text}),
                );
                if i == 1 {
                    let annotation = json!({"type":"url_citation","url":"https://example.org/weather","title":"Weather","start_index":0,"end_index":8});
                    append(
                        &mut events,
                        "response.output_text.annotation.added",
                        json!({"output_index":i,"item_id":items[i]["id"],"content_index":0,"annotation_index":0,"annotation":annotation}),
                    );
                    part["annotations"] = json!([annotation]);
                }
                part["text"] = json!(text);
                append(
                    &mut events,
                    &format!("{text_events}.done"),
                    json!({"output_index":i,"item_id":items[i]["id"],index_field:0,"text":text}),
                );
                append(
                    &mut events,
                    &format!("{part_events}.done"),
                    json!({"output_index":i,"item_id":items[i]["id"],index_field:0,"part":part}),
                );
                items[i][field] = json!([part]);
            }
        }
        if i == 0 {
            items[i]["encrypted_content"] = json!("opaque-continuation");
        } else {
            items[i]["status"] = json!("completed");
        }
        append(
            &mut events,
            "response.output_item.done",
            json!({"output_index":i,"item":items[i]}),
        );
    }
    let mut r = response("completed", json!(items));
    r["usage"] = json!({"input_tokens":10,"output_tokens":12,"total_tokens":22,"output_tokens_details":{"reasoning_tokens":4}});
    append(&mut events, "response.completed", json!({"response":r}));
    events
}
