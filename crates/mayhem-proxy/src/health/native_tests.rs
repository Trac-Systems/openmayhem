use super::*;
use serde_json::json;
fn d(n: u64) -> Digest {
    Digest::new(format!("{n:064x}")).unwrap()
}
fn bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
        "normalizer":null,"pre_tokenizer":{"type":"WhitespaceSplit"},"post_processor":null,"decoder":null,
        "model":{"type":"WordLevel","vocab":{"[UNK]":0,"one":1,"two":2,"three":3,"four":4,"five":5,"你好":6},"unk_token":"[UNK]"}})).unwrap()
}
fn limits() -> Limits {
    Limits {
        artifact_bytes: 1024 * 1024,
        output_bytes: 1024 * 1024,
        channels: 16,
        workers: 1,
        minimum_tokens: 2,
    }
}
struct FixtureSource {
    source: Source,
    _directory: tempfile::TempDir,
}
impl std::ops::Deref for FixtureSource {
    type Target = Source;
    fn deref(&self) -> &Source {
        &self.source
    }
}
fn isolate(source: Source) -> FixtureSource {
    // Unit semantic cases use the same real contained binary as the integration
    // tests. Cargo's package tests build it; focused --lib runs build it first.
    let executable = std::env::current_exe().unwrap();
    let program = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("mayhem-proxy-worker");
    let directory = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let pool = Pool::new(
        program,
        directory.path(),
        crate::worker::host::PoolLimits {
            max_children: 1,
            max_buffer_bytes: 8 * 1024 * 1024,
            startup_timeout: Duration::from_secs(5),
            processing_timeout: Duration::from_secs(5),
        },
    )
    .unwrap();
    source.validate(&pool).unwrap();
    FixtureSource {
        source,
        _directory: directory,
    }
}
fn source() -> FixtureSource {
    let b = bytes();
    isolate(
        Source::from_bytes(
            &b,
            Digest::new(blake3::hash(&b).to_hex().as_str()).unwrap(),
            d(1),
            d(2),
            limits(),
        )
        .unwrap(),
    )
}
fn delta(text: &str) -> Value {
    json!({"choices":[{"index":0,"delta":{"content":text}}]})
}

#[test]
fn pinned_tokenizer_rejects_wrong_data_padding_truncation_dropout_and_unbounded_limits() {
    let b = bytes();
    let digest = Digest::new(blake3::hash(&b).to_hex().as_str()).unwrap();
    assert!(Source::from_bytes(&b, d(99), d(1), d(2), limits()).is_err());
    for key in ["padding", "truncation", "dropout"] {
        let mut v: Value = serde_json::from_slice(&b).unwrap();
        if key == "dropout" {
            v["model"][key] = json!(0.5)
        } else {
            v[key] = json!({})
        }
        let changed = serde_json::to_vec(&v).unwrap();
        let hash = Digest::new(blake3::hash(&changed).to_hex().as_str()).unwrap();
        assert!(engine::load(&changed, &hash, limits()).is_err());
    }
    let mut bad = limits();
    bad.output_bytes = usize::MAX;
    assert!(Source::from_bytes(&b, digest, d(1), d(2), bad).is_err());
    let s = source();
    assert!(s.matches(&d(1), &d(2)));
    assert!(!s.matches(&d(2), &d(1)));
}

#[tokio::test]
async fn exact_counts_span_chunks_utf8_and_ignore_reported_usage_and_transport_metadata() {
    let s = source();
    let mut c = s.capture().unwrap();
    let first = Instant::now();
    c.delta(&delta("one "), first);
    c.delta(
        &json!({"choices":[],"usage":{"completion_tokens":999999}}),
        first + Duration::from_millis(1),
    );
    c.delta(&delta("two 你好 three"), first + Duration::from_millis(100));
    let e = c.finish().await.unwrap();
    assert_eq!(e.tokens, 3);
    assert_eq!(e.last.duration_since(e.first), Duration::from_millis(100));
    assert!(s.capture().is_some());
}

#[tokio::test]
async fn one_buffered_batch_short_labels_and_exceeded_bounds_never_certify_speed() {
    let s = source();
    let at = Instant::now();
    for mode in [
        "same_batch",
        "short",
        "bytes",
        "channels",
        "backwards",
        "buffered_part",
    ] {
        let mut c = s.capture().unwrap();
        if mode == "bytes" {
            c.limits.output_bytes = 4
        }
        if mode == "channels" {
            c.limits.channels = 1
        }
        c.delta(&delta("one "), at);
        match mode {
            "same_batch"=>c.delta(&delta("two three"),at),
            "short"=>c.delta(&delta("two"),at+Duration::from_secs(1)),
            "backwards"=>c.delta(&delta("two three"),at-Duration::from_millis(1)),
            "buffered_part"=>c.delta(&json!({"type":"response.content_part.added","part":{"type":"output_text","text":"hidden buffer"}}),at),
            "channels"=>c.delta(&json!({"choices":[{"index":0,"delta":{"reasoning":"two three"}}]}),at+Duration::from_secs(1)),
            _=>c.delta(&delta("two three"),at+Duration::from_secs(1)),
        }
        assert!(c.finish().await.is_none(), "{mode}");
    }
}

#[tokio::test]
async fn bpe_merge_crossing_first_boundary_is_not_counted_twice_or_as_new_work() {
    let b=serde_json::to_vec(&json!({"version":"1.0","truncation":null,"padding":null,"added_tokens":[],
        "normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,
        "model":{"type":"BPE","dropout":null,"unk_token":null,"continuing_subword_prefix":null,"end_of_word_suffix":null,"fuse_unk":false,"byte_fallback":false,
            "vocab":{"a":0,"b":1,"ab":2,"c":3,"d":4,"cd":5,"e":6,"f":7,"ef":8," ":9},"merges":[["a","b"],["c","d"],["e","f"]]}})).unwrap();
    let s = isolate(
        Source::from_bytes(
            &b,
            Digest::new(blake3::hash(&b).to_hex().as_str()).unwrap(),
            d(1),
            d(2),
            limits(),
        )
        .unwrap(),
    );
    let mut c = s.capture().unwrap();
    let first = Instant::now();
    c.delta(&delta("a"), first);
    c.delta(&delta("bcd ef"), first + Duration::from_millis(100));
    assert_eq!(c.finish().await.unwrap().tokens, 3); // cd, space, ef; ab straddles the boundary.
}

#[tokio::test]
async fn reasoning_aliases_tool_arguments_completions_and_responses_are_counted_by_separate_channels(
) {
    let s = source();
    let at = Instant::now();
    let mut c = s.capture().unwrap();
    c.delta(&delta("one "), at);
    c.delta(&json!({"choices":[{"index":0,"delta":{"reasoning":"two three ","reasoning_content":"two three ","tool_calls":[{"index":0,"function":{"name":"arbitrary repeated name", "arguments":"four five"}}]}}]}),at+Duration::from_millis(10));
    assert_eq!(c.finish().await.unwrap().tokens, 4);
    let mut c = s.capture().unwrap();
    c.delta(&json!({"choices":[{"index":0,"text":"one "}]}), at);
    c.delta(
        &json!({"choices":[{"index":0,"text":"two three"}]}),
        at + Duration::from_millis(10),
    );
    assert_eq!(c.finish().await.unwrap().tokens, 2);
    for kind in [
        "response.output_text.delta",
        "response.reasoning_text.delta",
        "response.reasoning_summary_text.delta",
        "response.refusal.delta",
        "response.function_call_arguments.delta",
    ] {
        let mut c = s.capture().unwrap();
        c.delta(
            &json!({"type":kind,"output_index":0,"content_index":0,"delta":"one "}),
            at,
        );
        c.delta(
            &json!({"type":kind,"output_index":0,"content_index":0,"delta":"two three"}),
            at + Duration::from_millis(10),
        );
        assert_eq!(c.finish().await.unwrap().tokens, 2, "{kind}");
    }
}

#[tokio::test]
async fn measurement_workers_are_bounded_and_pending_delivery_disqualifies_only_telemetry() {
    let s = source();
    let mut c = s.capture().unwrap();
    assert!(s.capture().is_none());
    let at = Instant::now();
    c.delta(&delta("one "), at);
    c.delta(&delta("two three"), at + Duration::from_secs(1));
    let flag = c.backpressure();
    assert_eq!(
        delivery(
            async {
                tokio::task::yield_now().await;
                17
            },
            Some(flag)
        )
        .await,
        17
    );
    assert!(c.finish().await.is_none());
    assert!(s.capture().is_some());
}

#[tokio::test]
async fn many_stream_updates_use_bounded_metadata_and_one_completed_text_encoding() {
    let s = source();
    let mut c = s.capture().unwrap();
    let at = Instant::now();
    for i in 0..10_000 {
        c.delta(&delta("one "), at + Duration::from_micros(i));
    }
    assert_eq!(c.fields.len(), 1);
    assert_eq!(c.bytes, 40_000);
    assert_eq!(c.finish().await.unwrap().tokens, 9_999);
}
