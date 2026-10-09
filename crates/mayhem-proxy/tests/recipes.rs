use mayhem_proto::proxy::ProxyEndpoint;
use mayhem_proxy::{
    connector::failure::Code,
    endpoint::Adapter,
    recipe::{Signed, Transform},
};
use serde_json::{json, Value};
#[path = "support/recipes.rs"]
mod support;

#[test]
fn signed_import_export_preview_and_legacy_identity_are_stable() {
    for endpoint in [
        ProxyEndpoint::Chat,
        ProxyEndpoint::Completions,
        ProxyEndpoint::Responses,
        ProxyEndpoint::Decisions,
    ] {
        let plain = support::base(endpoint);
        let before = serde_json::to_value(plain.snapshot()).unwrap();
        assert_eq!(before["version"], 1);
        assert!(before.get("recipe").is_none());
        let restored = Adapter::restore(serde_json::from_value(before.clone()).unwrap()).unwrap();
        assert_eq!(plain.recipe_hash(), restored.recipe_hash());
        assert_eq!(before, serde_json::to_value(restored.snapshot()).unwrap());
        let recipe = support::recipe(endpoint);
        recipe.validate().unwrap();
        let imported = Signed::import(&recipe.export().unwrap()).unwrap();
        assert_eq!(
            recipe.recipe.digest().unwrap(),
            imported.recipe.digest().unwrap()
        );
        let adapter = plain.with_recipe(imported).unwrap();
        let fixture = &recipe.recipe.fixtures[0];
        let preview = adapter
            .preview(&fixture.request, &fixture.upstream_response)
            .unwrap();
        assert_eq!(preview["upstream_request"]["engine"], "private-model");
        let common = support::base(endpoint)
            .prepare_json(&serde_json::to_vec(&fixture.request).unwrap())
            .unwrap()
            .decode_json(fixture.normalized_response.clone(), "recipe_preview", 0)
            .unwrap();
        assert_eq!(preview["normalized_result"], common.body);
        assert_eq!(
            preview["observed_usage"],
            serde_json::to_value(common.observed_usage).unwrap()
        );
        let snapshot = adapter.snapshot();
        assert_eq!(snapshot.version, 2);
        let restored = Adapter::restore(snapshot).unwrap();
        assert_eq!(adapter.recipe_hash(), restored.recipe_hash());
        if let Ok(path) = std::env::var("PROXY_RECIPE_FIXTURE_DIR") {
            std::fs::write(
                std::path::Path::new(&path).join(format!("{endpoint:?}.json")),
                recipe.export().unwrap(),
            )
            .unwrap();
            std::fs::write(std::path::Path::new(&path).join(format!("{endpoint:?}-preview.json")),serde_json::to_vec_pretty(&json!({"adapter":support::base(endpoint).snapshot(),"request":fixture.request,"response":fixture.upstream_response})).unwrap()).unwrap();
        }
    }
}
#[test]
fn tampering_abi_network_and_unknown_recipe_fields_are_rejected() {
    let signed = support::recipe(ProxyEndpoint::Chat);
    let value = serde_json::to_value(&signed).unwrap();
    for (pointer, change) in [
        ("/recipe/revision", json!(2)),
        ("/signature", json!("00".repeat(64))),
        ("/recipe/publisher", json!("01".repeat(32))),
        ("/recipe/abi_max", json!(2)),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = change;
        assert!(Signed::import(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    for key in [
        "url",
        "headers",
        "authentication",
        "code",
        "eval",
        "metering",
        "retry",
        "poll",
    ] {
        let mut bad = value.clone();
        bad["recipe"][key] = json!("forbidden");
        assert!(Signed::import(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut legacy = serde_json::to_value(support::base(ProxyEndpoint::Chat).snapshot()).unwrap();
    legacy["recipe"] = value;
    assert!(Adapter::restore(serde_json::from_value(legacy).unwrap()).is_err());
    assert!(Signed::import(&vec![b' '; 65_537]).is_err());
}
#[test]
fn unrepresented_controls_nested_fields_and_streaming_fail_before_dispatch() {
    let recipe = support::recipe(ProxyEndpoint::Chat);
    let original = recipe.recipe.fixtures[0].request.clone();
    let adapter = support::base(ProxyEndpoint::Chat)
        .with_recipe(recipe)
        .unwrap();
    for key in ["seed", "frequency_penalty"] {
        let mut body = original.clone();
        body[key] = json!(1);
        assert!(adapter
            .prepare_json(&serde_json::to_vec(&body).unwrap())
            .is_err());
    }
    let mut body = original.clone();
    body["messages"][0]["extra_instruction"] = json!("hidden");
    assert!(adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .is_err());
    body = original;
    body["stream"] = json!(true);
    assert!(
        matches!(adapter.prepare_stream(&serde_json::to_vec(&body).unwrap()),Err(mayhem_proxy::endpoint::Error::Request(f)) if f.code==Code::UnsupportedControl)
    );
}
#[test]
fn bounded_transforms_reject_collisions_expansion_missing_and_ambiguous_outcomes() {
    let signed = support::recipe(ProxyEndpoint::Chat);
    let fixture = signed.recipe.fixtures[0].clone();
    let adapter = support::base(ProxyEndpoint::Chat)
        .with_recipe(signed.clone())
        .unwrap();
    for raw in [
        json!({"state":"ok","fault":{"secret":"should not leak"},"results":[]}),
        json!({"state":"future","fault":null}),
        json!({"state":"ok"}),
        json!({"fault":null}),
    ] {
        assert!(adapter.preview(&fixture.request, &raw).is_err());
    }
    let mut recipe = signed.recipe.clone();
    if let Transform::Object { fields } = &mut recipe.request {
        fields.get_mut("model").unwrap().target = vec!["dialog".into()];
    }
    assert!(recipe.signing_bytes().is_err());
    let mut recipe = signed.recipe.clone();
    recipe.max_json_bytes = 10;
    assert!(recipe.signing_bytes().is_err());
    let mut body = fixture.request.clone();
    body["messages"] = Value::Array(vec![body["messages"][0].clone(); 65]);
    assert!(adapter
        .prepare_json(&serde_json::to_vec(&body).unwrap())
        .is_err());
    let mut raw = fixture.upstream_response;
    raw["results"] = Value::Array(vec![raw["results"][0].clone(); 9]);
    assert!(adapter.preview(&fixture.request, &raw).is_err());
}
#[test]
fn common_tool_and_decision_semantics_survive_translation() {
    let recipe = support::recipe(ProxyEndpoint::Chat);
    let fixture = recipe.recipe.fixtures[0].clone();
    let adapter = support::base(ProxyEndpoint::Chat)
        .with_recipe(recipe)
        .unwrap();
    let mut body = fixture.request;
    body["tools"] = json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}}}]);
    body["tool_choice"] = json!("required");
    let mut response = fixture.upstream_response;
    response["results"][0]["text"] = Value::Null;
    response["results"][0]["stop"] = json!("tools");
    response["results"][0]["calls"] = json!([{"id":"c1","type":"function","function":{"name":"lookup","arguments":"{ \"key\": \"abc\" }"}}]);
    let valid = adapter.preview(&body, &response).unwrap();
    assert_eq!(
        valid["normalized_result"]["choices"][0]["message"]["tool_calls"][0]["function"]
            ["arguments"],
        "{ \"key\": \"abc\" }"
    );
    response["results"][0]["calls"][0]["function"]["arguments"] = json!("[]");
    assert!(adapter.preview(&body, &response).is_err());
    let recipe = support::recipe(ProxyEndpoint::Decisions);
    let fixture = recipe.recipe.fixtures[0].clone();
    let adapter = support::base(ProxyEndpoint::Decisions)
        .with_recipe(recipe)
        .unwrap();
    let mut response = fixture.upstream_response;
    response["labels"]["q"]["choice"] = json!("other");
    assert!(adapter.preview(&fixture.request, &response).is_err());
}

#[test]
fn reusable_signed_examples_and_preview_files_roundtrip_without_private_configuration() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipes");
    for name in ["Chat", "Completions", "Responses", "Decisions"] {
        let signed = Signed::load(&root.join(format!("{name}.json"))).unwrap();
        let result = signed
            .preview_file(&root.join(format!("{name}-preview.json")))
            .unwrap();
        assert_eq!(result["semantic_verification"], "requires_supervised_probe");
        assert!(Signed::import(&signed.export().unwrap()).is_ok());
        let review = serde_json::to_string(&signed.review().unwrap()).unwrap();
        for private in [
            "upstream_request",
            "Hello λ",
            "private-model",
            "authentication",
            "base_url",
        ] {
            assert!(!review.contains(private));
        }
    }
}
#[test]
fn program_resource_budgets_and_injective_request_mapping_are_enforced() {
    let original = support::recipe(ProxyEndpoint::Chat);
    let mut value = serde_json::to_value(&original.recipe).unwrap();
    value["request"]["fields"]["messages"]["transform"]["item"]["fields"]["role"]["transform"]
        ["values"]["assistant"] = json!("human");
    assert!(
        serde_json::from_value::<mayhem_proxy::recipe::Recipe>(value)
            .unwrap()
            .signing_bytes()
            .is_err()
    );
    let mut value = serde_json::to_value(&original.recipe).unwrap();
    value["response"]["fields"]["usage"] =
        json!({"optional":false,"value":{"kind":"literal","value":1}});
    assert!(
        serde_json::from_value::<mayhem_proxy::recipe::Recipe>(value)
            .unwrap()
            .signing_bytes()
            .is_err()
    );
    let mut body = original.recipe.fixtures[0].request.clone();
    let mut nested = json!({"leaf":1});
    for _ in 0..25 {
        nested = json!({"nested":nested});
    }
    body["tools"] = nested;
    assert!(original.recipe.map_request(&body).is_err());
    body["tools"] = json!("x".repeat(65536));
    assert!(original.recipe.map_request(&body).is_err());
    for endpoint in ["embeddings", "image", "workflow", "audio"] {
        let mut value = serde_json::to_value(&original).unwrap();
        value["recipe"]["endpoint"] = json!(endpoint);
        assert!(Signed::import(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    let mut value = serde_json::to_value(&original.recipe).unwrap();
    let mut node = json!({"kind":"identity"});
    for _ in 0..13 {
        node = json!({"kind":"array","max_items":1,"item":node});
    }
    value["request"]["fields"]["tools"]["transform"] = node;
    assert!(
        serde_json::from_value::<mayhem_proxy::recipe::Recipe>(value)
            .unwrap()
            .signing_bytes()
            .is_err()
    );
}
