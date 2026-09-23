//! Shared quote/settlement vectors for video workflow metering.
//!
//! `catalog/comfy/tests/video-metering-vectors-v1.json` is the single source of
//! truth for both this suite and the website estimator suite. Every case runs
//! through the real graph derivation (`mayhem_proto::derive_comfy_workflow`) and
//! the real settlement arithmetic (`mayhem_gateway::pricing::priced_usage_au`);
//! nothing here reimplements the meter.
//!
//! Frames are the quoted duration frames (explicit `length`, else
//! `seconds * fps`). A worker that internally aligns to its own trained frame
//! grid, such as MiniMax H3's `17n + 5` rule, does not change the billed count.
//!
//! A price is `per_req_au + ceil(count * per_unit_au / granularity)`. The two H3
//! low-VRAM classes carry a fixed per-request component, so a 736x1280 clip
//! shorter than five seconds costs slightly more than it did before. Each vector
//! states that outcome in `price_rises_vs_old`, and the suite asserts that field
//! per case rather than a blanket "never more expensive".

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use mayhem_gateway::pricing::{priced_usage_au, RateMapEntry};
use mayhem_proto::{
    derive_comfy_workflow, ComfyWorkflowDerivationPolicy, MoneyAu, ReceiptUsage,
    USAGE_MEGAPIXEL_STEP, USAGE_PIXEL_FRAME,
};
use serde_json::{json, Value};

const ATTO_PER_MICRO: MoneyAu = 1_000_000_000_000;

/// The two classes whose tariff is a fixed per-request component plus a
/// per-megapixel-frame rate, rather than a mechanical conversion of the rate
/// they replaced.
const H3_LOWVRAM_CLASSES: [&str; 2] = [
    "video.minimax_h3.lowvram_t2v_i2v",
    "video.minimax_h3.lowvram_r2v",
];

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn read_json(relative: &str) -> Value {
    let path = repo_path(relative);
    let bytes = fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

fn vectors() -> Value {
    read_json("catalog/comfy/tests/video-metering-vectors-v1.json")
}

fn u64_field(case: &Value, key: &str) -> u64 {
    case.get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("vector case is missing numeric {key}"))
}

fn decimal_field(case: &Value, key: &str) -> MoneyAu {
    case.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("vector case is missing decimal {key}"))
        .parse()
        .unwrap_or_else(|err| panic!("vector case {key} is not a decimal integer: {err}"))
}

/// Permissive derivation caps. Per-class admission caps are enforced separately;
/// these vectors exercise the meter, not admission.
fn metering_policy(pricing_unit: &str) -> ComfyWorkflowDerivationPolicy {
    ComfyWorkflowDerivationPolicy {
        whitelisted_nodes: [
            "MiniMaxH3Easy",
            "BasicScheduler",
            "CreateVideo",
            "SaveVideo",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        pricing_unit: Some(pricing_unit.to_owned()),
        max_width: 8_192,
        max_height: 8_192,
        max_frames: 4_096,
        max_duration_seconds: 600,
        max_steps: 1_000,
        max_artifacts: 64,
        ..ComfyWorkflowDerivationPolicy::default()
    }
}

/// A video graph carrying exactly the metrics a vector case declares.
fn video_graph(case: &Value) -> Value {
    let mut inputs = serde_json::Map::new();
    inputs.insert("width".to_owned(), json!(u64_field(case, "width")));
    inputs.insert("height".to_owned(), json!(u64_field(case, "height")));
    inputs.insert("fps".to_owned(), json!(u64_field(case, "fps")));
    inputs.insert("seconds".to_owned(), json!(u64_field(case, "seconds")));
    inputs.insert(
        "batch_size".to_owned(),
        json!(u64_field(case, "artifact_count")),
    );
    // Only set an explicit frame count when the case declares one that the
    // seconds * fps derivation would not produce.
    let seconds_frames = u64_field(case, "seconds") * u64_field(case, "fps");
    let frames = u64_field(case, "frames");
    if frames != seconds_frames {
        inputs.insert("length".to_owned(), json!(frames));
    }
    json!({
        "1": {"class_type": "MiniMaxH3Easy", "inputs": Value::Object(inputs)},
        "2": {"class_type": "BasicScheduler", "inputs": {"steps": 4}},
        "3": {"class_type": "CreateVideo", "inputs": {"images": ["1", 0], "fps": u64_field(case, "fps")}},
        "4": {"class_type": "SaveVideo", "inputs": {"video": ["3", 0], "filename_prefix": "metering-vector"}}
    })
}

fn rate_map_from(rate: &Value) -> Vec<RateMapEntry> {
    vec![RateMapEntry {
        unit: rate["unit"].as_str().expect("rate unit").to_owned(),
        per_unit_au: rate["per_unit_au"]
            .as_str()
            .expect("rate per_unit_au")
            .parse()
            .expect("rate per_unit_au is a decimal integer"),
        granularity: rate["granularity"].as_u64().expect("rate granularity"),
    }]
}

fn micro_from_au(au: MoneyAu) -> MoneyAu {
    au.div_ceil(ATTO_PER_MICRO)
}

#[test]
fn video_metering_vectors_match_derivation_and_pricing() {
    let vectors = vectors();
    let cases = vectors["cases"].as_array().expect("vector cases");
    assert_eq!(cases.len(), 34, "vector file must carry all 34 cases");

    for case in cases {
        let label = case["label"].as_str().expect("case label");
        let expected_count = u64_field(case, "pixel_frame_count");
        let rate = &case["new_rate"];
        assert_eq!(
            rate["unit"].as_str(),
            Some(USAGE_PIXEL_FRAME),
            "{label}: vectors must price in pixel_frame"
        );
        assert_eq!(
            rate["granularity"].as_u64(),
            Some(1_000_000),
            "{label}: pixel_frame granularity must be 1_000_000"
        );

        let derivation =
            derive_comfy_workflow(&video_graph(case), &metering_policy(USAGE_PIXEL_FRAME))
                .unwrap_or_else(|err| panic!("{label}: derivation failed: {err}"));

        assert!(
            derivation
                .outcome_spec
                .output_modalities
                .iter()
                .any(|modality| modality == "video"),
            "{label}: case must derive a video outcome"
        );
        assert_eq!(
            derivation.outcome_spec.width,
            Some(u64_field(case, "width")),
            "{label}: width"
        );
        assert_eq!(
            derivation.outcome_spec.height,
            Some(u64_field(case, "height")),
            "{label}: height"
        );
        assert_eq!(
            derivation.outcome_spec.frames,
            Some(u64_field(case, "frames")),
            "{label}: frames"
        );
        assert_eq!(
            derivation.outcome_spec.artifact_count,
            u64_field(case, "artifact_count"),
            "{label}: artifact_count"
        );

        // Exact integer pixel-frames, no rounding anywhere in the count.
        assert_eq!(
            derivation.quoted_usage.get(USAGE_PIXEL_FRAME),
            expected_count,
            "{label}: pixel_frame_count"
        );
        assert_eq!(
            derivation.quoted_usage.units().len(),
            1,
            "{label}: a priced workflow quotes exactly one unit"
        );

        let rate_map = rate_map_from(rate);
        let per_req_au = decimal_field(case, "new_per_req_au");
        let priced = priced_usage_au(&rate_map, per_req_au, 0, &derivation.quoted_usage);
        assert_eq!(
            priced,
            decimal_field(case, "new_price_au"),
            "{label}: new_price_au"
        );
        assert_eq!(
            micro_from_au(priced),
            decimal_field(case, "new_price_micro"),
            "{label}: new_price_micro"
        );

        // Some shapes cost more than they did: a 736x1280 H3 low-VRAM clip under
        // five seconds now carries the fixed per-request component. Each vector
        // states which way its own case went, and that is what gets asserted.
        let expected_rise = case["price_rises_vs_old"]
            .as_bool()
            .unwrap_or_else(|| panic!("{label}: vector is missing price_rises_vs_old"));
        let old_priced = decimal_field(case, "old_price_au");
        assert_eq!(
            priced > old_priced,
            expected_rise,
            "{label}: new {priced} au vs pre-fix {old_priced} au contradicts price_rises_vs_old"
        );
    }

    // Guard the shape of the exception itself, so a future tariff edit that
    // widens it cannot pass quietly.
    let rising = cases
        .iter()
        .filter(|case| case["price_rises_vs_old"].as_bool() == Some(true))
        .map(|case| case["label"].as_str().expect("case label"))
        .collect::<Vec<_>>();
    assert_eq!(
        rising,
        vec![
            "lowvram 720P portrait 736x1280 1s",
            "lowvram 720P portrait 736x1280 4s (shorter than 5 s)",
        ],
        "only the two short 736x1280 H3 low-VRAM clips may cost more than before"
    );
}

#[test]
fn vector_tariffs_match_the_published_grid_and_catalog() {
    /// Everything that decides what a class charges: the priced unit, its rate,
    /// and the fixed per-request fee. `per_req_au` is a second, independent
    /// money field, so deriving it from the old rate would prove nothing.
    #[derive(Debug, Eq, PartialEq)]
    struct Tariff {
        unit: String,
        per_unit_au: MoneyAu,
        granularity: u64,
        per_req_au: MoneyAu,
        min_session_au: MoneyAu,
    }

    fn tariff_from(source: &Value, pricing_unit: &str, label: &str) -> Tariff {
        let entry = source["rate_map"]
            .as_array()
            .unwrap_or_else(|| panic!("{label}: rate_map must be an array"))
            .iter()
            .find(|entry| entry["unit"].as_str() == Some(pricing_unit))
            .unwrap_or_else(|| panic!("{label}: rate_map must price {pricing_unit}"));
        Tariff {
            unit: entry["unit"].as_str().expect("unit").to_owned(),
            per_unit_au: decimal_field(entry, "per_unit_au"),
            granularity: entry["granularity"].as_u64().expect("granularity"),
            per_req_au: decimal_field(source, "per_req_au"),
            min_session_au: decimal_field(source, "min_session_au"),
        }
    }

    let grid = read_json("catalog/comfy/outcome-classes-v1.json");
    let catalog = read_json("catalog/models.json");

    let mut catalog_tariffs = BTreeMap::<String, Tariff>::new();
    for model in catalog["models"].as_array().expect("catalog models") {
        let Some(definition) = model.pointer("/workflow/outcome_class_definition") else {
            continue;
        };
        let class_id = definition["class_id"]
            .as_str()
            .expect("class id")
            .to_owned();
        let pricing_unit = definition["pricing_unit"].as_str().expect("pricing unit");
        catalog_tariffs.insert(
            class_id.clone(),
            tariff_from(
                &model["price_ref_au"],
                pricing_unit,
                &format!("{class_id} price_ref_au"),
            ),
        );
    }

    let mut grid_tariffs = BTreeMap::<String, Tariff>::new();
    for row in grid["classes"].as_array().expect("grid classes") {
        let class_id = row["class_id"].as_str().expect("class id").to_owned();
        let pricing_unit = row["pricing_unit"].as_str().expect("pricing unit");

        // `megapixel_step` rounds a frame's area up to a whole megapixel, so no
        // video class may use it. `frame` stays valid for lanes such as
        // `video.lipsync` that bill per frame and never per pixel.
        if row["media"].as_str() == Some("video") {
            assert_ne!(
                row["pricing_unit"].as_str(),
                Some(USAGE_MEGAPIXEL_STEP),
                "{class_id}: megapixel_step is image-only"
            );
            for entry in row["rate_map"].as_array().expect("rate map") {
                assert_ne!(
                    entry["unit"].as_str(),
                    Some(USAGE_MEGAPIXEL_STEP),
                    "{class_id}: megapixel_step is image-only"
                );
            }
        }

        let tariff = tariff_from(row, pricing_unit, &format!("outcome grid {class_id}"));
        if let Some(published) = catalog_tariffs.get(&class_id) {
            assert_eq!(
                &tariff, published,
                "{class_id}: outcome grid tariff does not match the shipped catalog price_ref_au"
            );
        }
        grid_tariffs.insert(class_id, tariff);
    }

    let vectors = vectors();
    for case in vectors["cases"].as_array().expect("vector cases") {
        let class_id = case["class_id"].as_str().expect("class id");
        let published = grid_tariffs
            .get(class_id)
            .unwrap_or_else(|| panic!("{class_id} is missing from the outcome-class grid"));
        let rate = &case["new_rate"];

        // The vector's whole tariff, unit + rate + fixed fee, must be the one
        // that is actually published for that class.
        assert_eq!(
            rate["unit"].as_str(),
            Some(published.unit.as_str()),
            "{class_id}: unit"
        );
        assert_eq!(
            decimal_field(rate, "per_unit_au"),
            published.per_unit_au,
            "{class_id}: per_unit_au"
        );
        assert_eq!(
            rate["granularity"].as_u64(),
            Some(published.granularity),
            "{class_id}: granularity"
        );
        assert_eq!(
            decimal_field(case, "new_per_req_au"),
            published.per_req_au,
            "{class_id}: per_req_au"
        );

        // The six generic video classes keep the mechanical conversion of the
        // rate they replaced and charge no fixed fee. The two H3 low-VRAM
        // classes carry their own tariff, so they are checked against the
        // published numbers above and not against the old rate.
        let old_rate = &case["old_rate"];
        assert_eq!(old_rate["unit"].as_str(), Some(USAGE_MEGAPIXEL_STEP));
        assert_eq!(old_rate["granularity"].as_u64(), Some(1_000));
        if !H3_LOWVRAM_CLASSES.contains(&class_id) {
            assert_eq!(
                published.per_req_au, 0,
                "{class_id}: a mechanically converted class charges no fixed fee"
            );
            assert_eq!(
                published.per_unit_au,
                decimal_field(old_rate, "per_unit_au").div_ceil(1_000),
                "{class_id}: rate is not the mechanical conversion of the old rate"
            );
        } else {
            assert!(
                published.per_req_au > 0,
                "{class_id}: the H3 low-VRAM tariff carries a fixed per-request component"
            );
        }
        assert_eq!(
            published.min_session_au, 0,
            "{class_id}: no video class sets a session minimum"
        );
    }

    // The H3 low-VRAM tariff is anchored so that 736x1280 at five seconds and
    // 24 fps prices at exactly 125,000 micro-USD ($0.125). If that lands
    // anywhere else the tariff was mistyped.
    let anchor = grid_tariffs
        .get("video.minimax_h3.lowvram_t2v_i2v")
        .expect("H3 low-VRAM class");
    let anchor_rate = vec![RateMapEntry {
        unit: anchor.unit.clone(),
        per_unit_au: anchor.per_unit_au,
        granularity: anchor.granularity,
    }];
    let anchor_usage = ReceiptUsage::from_units([(USAGE_PIXEL_FRAME, 736 * 1_280 * 120)]);
    assert_eq!(
        micro_from_au(priced_usage_au(
            &anchor_rate,
            anchor.per_req_au,
            anchor.min_session_au,
            &anchor_usage,
        )),
        125_000,
        "736x1280 for five seconds at 24 fps must cost exactly 125000 micro-USD"
    );
}

#[test]
fn video_outcomes_priced_in_megapixel_step_fail_closed() {
    let case = json!({
        "width": 736,
        "height": 1280,
        "fps": 24,
        "seconds": 5,
        "frames": 120,
        "artifact_count": 1
    });
    let err = derive_comfy_workflow(&video_graph(&case), &metering_policy(USAGE_MEGAPIXEL_STEP))
        .expect_err("a video outcome priced in megapixel_step must fail closed");
    let message = err.to_string();
    assert!(
        message.contains("megapixel_step") && message.contains("pixel_frame"),
        "admission error must name both units: {message}"
    );
}

#[test]
fn image_outcomes_priced_in_pixel_frame_fail_closed() {
    let policy = ComfyWorkflowDerivationPolicy {
        whitelisted_nodes: ["EmptyLatentImage", "KSampler", "SaveImage"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        pricing_unit: Some(USAGE_PIXEL_FRAME.to_owned()),
        max_width: 4_096,
        max_height: 4_096,
        ..ComfyWorkflowDerivationPolicy::default()
    };
    let graph = json!({
        "1": {"class_type": "EmptyLatentImage", "inputs": {"width": 1024, "height": 1024, "batch_size": 1}},
        "2": {"class_type": "KSampler", "inputs": {"steps": 8, "latent_image": ["1", 0]}},
        "3": {"class_type": "SaveImage", "inputs": {"images": ["2", 0], "filename_prefix": "metering-vector"}}
    });
    let err = derive_comfy_workflow(&graph, &policy)
        .expect_err("an image outcome priced in pixel_frame must fail closed");
    assert!(
        err.to_string().contains("video-only"),
        "unexpected error: {err}"
    );
}

#[test]
fn unknown_pricing_units_fail_closed_instead_of_billing_one_unit() {
    let policy = ComfyWorkflowDerivationPolicy {
        pricing_unit: Some("seller_second".to_owned()),
        ..metering_policy(USAGE_PIXEL_FRAME)
    };
    let case = json!({
        "width": 736,
        "height": 1280,
        "fps": 24,
        "seconds": 5,
        "frames": 120,
        "artifact_count": 1
    });
    let err = derive_comfy_workflow(&video_graph(&case), &policy)
        .expect_err("an unknown pricing unit must fail closed");
    assert!(
        err.to_string()
            .contains("unsupported pricing_unit seller_second"),
        "unexpected error: {err}"
    );
    // Guard the old defect directly: the fallback used to quote one unit.
    assert_ne!(
        ReceiptUsage::from_units([("seller_second", 1)]),
        ReceiptUsage::default()
    );
}
