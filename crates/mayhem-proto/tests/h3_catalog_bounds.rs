use mayhem_proto::{derive_comfy_workflow, ComfyWorkflowCatalogPolicy};
use serde_json::{json, Value};

#[test]
fn published_h3_canvas_envelope_preserves_metering_and_rejects_oversize_graphs() {
    let catalog: Value =
        serde_json::from_str(include_str!("../../../catalog/models.json")).unwrap();
    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["model_id"] == "video.minimax_h3.lowvram_t2v_i2v")
        .unwrap();
    let policy: ComfyWorkflowCatalogPolicy =
        serde_json::from_value(model["workflow"].clone()).unwrap();
    let policy = policy.derivation_policy().unwrap();
    let canary: Value = serde_json::from_str(include_str!(
        "../../../catalog/canaries/canary-minimax-h3-lowvram-workflow-launch-v1.json"
    ))
    .unwrap();
    let mut graph = canary["prompts"][0]["workflow"].clone();
    graph["2"]["inputs"]["resolution"] = json!("custom");
    for (width, height) in [
        (960, 960),
        (800, 1184),
        (1184, 800),
        (832, 1120),
        (1120, 832),
        (736, 1280),
        (1280, 736),
        (1472, 640),
        (1472, 672),
        (992, 992),
        (256, 256),
        (1472, 256),
    ] {
        graph["2"]["inputs"]["width"] = json!(width);
        graph["2"]["inputs"]["height"] = json!(height);
        let derived = derive_comfy_workflow(&graph, &policy).unwrap();
        assert_eq!(derived.outcome_spec.width, Some(width));
        assert_eq!(derived.outcome_spec.height, Some(height));
        assert_eq!(derived.outcome_spec.frames, Some(240));
        assert_eq!(
            derived.quoted_usage.units().get("pixel_frame"),
            Some(&(width * height * 240))
        );
    }
    for (width, height) in [
        (1504, 640),
        (736, 1312),
        (1024, 1024),
        (1472, 704),
        (800, 1280),
        (1471, 640),
        (224, 256),
    ] {
        graph["2"]["inputs"]["width"] = json!(width);
        graph["2"]["inputs"]["height"] = json!(height);
        assert!(
            derive_comfy_workflow(&graph, &policy).is_err(),
            "accepted {width}x{height}"
        );
    }
}
