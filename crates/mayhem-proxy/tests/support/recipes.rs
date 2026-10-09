use ed25519_dalek::{Signer, SigningKey};
use mayhem_proto::{endpoint_family_contract_template, proxy::ProxyEndpoint};
use mayhem_proxy::{
    endpoint::{Adapter, Limits},
    recipe::{Recipe, Signed},
};
use serde_json::{json, Value};
pub fn limits() -> Limits {
    Limits {
        request_bytes: 1024 * 1024,
        response_bytes: 1024 * 1024,
        choices: 8,
        tools: 16,
        questions: 16,
        decision_options: 32,
    }
}
pub fn base(endpoint: ProxyEndpoint) -> Adapter {
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
        limits(),
    )
    .unwrap()
}
fn copy(path: &[&str]) -> Value {
    json!({"kind":"copy","path":path})
}
fn field(value: Value) -> Value {
    json!({"optional":false,"value":value})
}
fn optional(path: &[&str]) -> Value {
    json!({"optional":true,"value":copy(path)})
}
fn rename(target: &[&str]) -> Value {
    json!({"target":target,"transform":{"kind":"identity"}})
}
pub fn sign(recipe: Recipe) -> Signed {
    let key = SigningKey::from_bytes(&[93; 32]);
    let signature = key
        .sign(&recipe.signing_bytes().unwrap())
        .to_bytes()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    Signed { recipe, signature }
}
pub fn recipe(endpoint: ProxyEndpoint) -> Signed {
    let a = base(endpoint);
    let key = SigningKey::from_bytes(&[93; 32]);
    let publisher: String = key
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    let mut fields = serde_json::Map::new();
    for (k, path) in [
        ("model", vec!["engine"]),
        ("temperature", vec!["options", "heat"]),
        ("max_tokens", vec!["options", "limit"]),
        ("stream", vec!["options", "stream"]),
        ("tools", vec!["functions"]),
        ("tool_choice", vec!["selection"]),
        ("response_format", vec!["format"]),
        ("n", vec!["count"]),
        ("store", vec!["retain"]),
    ] {
        fields.insert(k.into(), rename(&path));
    }
    let (request, upstream_request, raw, normalized, response) = match endpoint {
        ProxyEndpoint::Chat => {
            fields.insert("messages".into(),json!({"target":["dialog"],"transform":{"kind":"array","max_items":64,"item":{"kind":"object","fields":{
                "role":{"target":["speaker"],"transform":{"kind":"enum","values":{"user":"human","assistant":"bot","system":"system","tool":"tool","developer":"developer"}}},
                "content":rename(&["text"]),"tool_calls":rename(&["calls"]),"tool_call_id":rename(&["call_ref"]),"name":rename(&["name"])
            }}}}));
            (
                json!({"model":"sample","messages":[{"role":"user","content":"Hello λ"}]}),
                json!({"engine":"sample","dialog":[{"speaker":"human","text":"Hello λ"}]}),
                json!({"state":"ok","fault":null,"job":"j1","results":[{"ordinal":0,"speaker":"bot","text":"Hi λ","stop":"done"}]}),
                json!({"id":"j1","choices":[{"index":0,"message":{"role":"assistant","content":"Hi λ"},"finish_reason":"stop"}]}),
                json!({"kind":"object","fields":{"id":field(copy(&["job"])),"choices":field(json!({"kind":"array","path":["results"],"max_items":8,"item":{"kind":"object","fields":{
                   "index":field(copy(&["ordinal"])),"finish_reason":field(json!({"kind":"enum","path":["stop"],"values":{"done":"stop","tools":"tool_calls","limit":"length","refused":"content_filter"}})),
                   "message":field(json!({"kind":"object","fields":{"role":field(json!({"kind":"enum","path":["speaker"],"values":{"bot":"assistant"}})),"content":field(copy(&["text"])),"tool_calls":optional(&["calls"]),"refusal":optional(&["refusal"]),"reasoning_content":optional(&["reasoning"])}}))
                }}}))}}),
            )
        }
        ProxyEndpoint::Decisions => {
            fields.insert("state".into(), rename(&["context"]));
            fields.insert("questions".into(), rename(&["queries"]));
            let questions = json!({"q":{"type":"choice","instructions":"classify","criteria":{"yes":"Yes","no":"No"}}});
            let answers = json!({"q":{"type":"choice","choice":"yes","probabilities":{"yes":0.75,"no":0.25}}});
            (
                json!({"model":"sample","state":"classify","questions":questions}),
                json!({"engine":"sample","context":"classify","queries":questions}),
                json!({"state":"ok","fault":null,"job":"j2","labels":answers}),
                json!({"id":"j2","answers":answers}),
                json!({"kind":"object","fields":{"id":field(copy(&["job"])),"answers":field(copy(&["labels"]))}}),
            )
        }
        ProxyEndpoint::Completions => {
            fields.insert("prompt".into(), rename(&["text"]));
            let choices = json!([{"index":0,"text":"Hi","finish_reason":"stop"}]);
            (
                json!({"model":"sample","prompt":"Hello"}),
                json!({"engine":"sample","text":"Hello"}),
                json!({"state":"ok","fault":null,"job":"j3","result":choices}),
                json!({"id":"j3","choices":choices}),
                json!({"kind":"object","fields":{"id":field(copy(&["job"])),"choices":field(copy(&["result"]))}}),
            )
        }
        ProxyEndpoint::Responses => {
            fields.insert("input".into(), rename(&["context"]));
            let out = json!([{"id":"m1","type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi","annotations":[],"logprobs":[]}],"status":"completed"}]);
            (
                json!({"model":"sample","input":"Hello"}),
                json!({"engine":"sample","context":"Hello"}),
                json!({"state":"ok","fault":null,"job":"j4","result":out}),
                json!({"id":"j4","status":"completed","output":out}),
                json!({"kind":"object","fields":{"id":field(copy(&["job"])),"status":field(json!({"kind":"literal","value":"completed"})),"output":field(copy(&["result"]))}}),
            )
        }
    };
    sign(serde_json::from_value(json!({"schema_version":1,"revision":1,"abi_min":1,"abi_max":1,"publisher":publisher,
        "endpoint":endpoint,"contract_hash":a.contract_hash(),"max_json_bytes":65536,"request":{"kind":"object","fields":fields},
        "outcome":{"path":["state"],"success":"ok","error_path":["fault"],"errors":{"busy":"busy","limited":"rate_limited","bad":"invalid_request"}},"response":response,
        "fixtures":[{"request":request,"upstream_request":upstream_request,"upstream_response":raw,"normalized_response":normalized}]})).unwrap())
}
