//! Explicit opt-in acceptance against an operator-approved running backend.
//! This exercises the actual connector, isolated decoder, protocol validation,
//! metering and retained result. Finance is local fixture data, never mainnet.
use super::*;
use serde::Deserialize;
use std::{io::Write, path::PathBuf, time::Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    connection_file: PathBuf,
    upstream_model: String,
    endpoint: ProxyEndpoint,
    request: Value,
    timeout_seconds: u64,
}

#[tokio::test]
#[ignore = "requires explicit operator-approved private configuration and real inference"]
async fn approved_live_backend_protocol_and_recovery() {
    assert_eq!(
        std::env::var("MAYHEM_PROXY_LIVE_APPROVED").as_deref(),
        Ok("1")
    );
    let path =
        PathBuf::from(std::env::var_os("MAYHEM_PROXY_LIVE_INPUT").expect("private input file"));
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.is_file() && meta.permissions().mode() & 0o077 == 0);
    assert!(meta.len() <= 64 * 1024);
    let input: Input = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!((1..=300).contains(&input.timeout_seconds));
    assert!(input.connection_file.is_absolute());
    let config = ConnectionConfig::load(&input.connection_file).unwrap();
    let f = Fixture::with_connection(
        config,
        input.endpoint,
        16 * 1024 * 1024,
        input.upstream_model,
    );
    let mut request = input.request;
    request["model"] = json!("public-model");
    if input.endpoint != ProxyEndpoint::Decisions {
        let limit = request
            .get("max_tokens")
            .or_else(|| request.get("max_output_tokens"))
            .or_else(|| request.get("max_completion_tokens"))
            .and_then(Value::as_u64)
            .expect("explicit bounded output required for live acceptance");
        assert!((1..=1024).contains(&limit));
    }
    let streaming = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let bytes = serde_json::to_vec(&request).unwrap();
    assert!(bytes.len() <= 64 * 1024);
    let record = f.prepare_kind(1, &bytes, ProxyRail::Fiat, streaming);
    let start = Instant::now();
    let mut first_output = None;
    let mut last_output = None;
    let mut events = 0usize;
    let cancellation = Cancellation::default();
    let call = async {
        if streaming {
            f.executor
                .execute_stream(&record.invocation, &bytes, &cancellation, |v| {
                    events += 1;
                    // Role-only and bookkeeping events are not a useful-output TTFT.
                    let useful =
                        v.get("choices")
                            .and_then(Value::as_array)
                            .is_some_and(|choices| {
                                choices.iter().any(|c| {
                                    let delta = &c["delta"];
                                    ["content", "reasoning_content", "reasoning"]
                                        .iter()
                                        .any(|k| delta[*k].as_str().is_some_and(|s| !s.is_empty()))
                                        || delta["tool_calls"]
                                            .as_array()
                                            .is_some_and(|a| !a.is_empty())
                                        || c["text"].as_str().is_some_and(|s| !s.is_empty())
                                })
                            })
                            || v.get("delta")
                                .and_then(Value::as_str)
                                .is_some_and(|s| !s.is_empty());
                    if useful {
                        let elapsed = start.elapsed();
                        first_output.get_or_insert(elapsed);
                        last_output = Some(elapsed);
                    }
                    async { Ok(()) }
                })
                .await
        } else {
            f.executor
                .execute_json(&record.invocation, &bytes, &cancellation)
                .await
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(input.timeout_seconds), call)
        .await
        .expect("operator test deadline exceeded; do not automatically resend")
        .expect("real backend did not pass proxy protocol validation");
    let completion = start.elapsed();
    assert_eq!(result.reply.body["model"], "public-model");
    assert!(result.reply.observed_usage.is_some());
    if streaming {
        assert!(events > 0 && first_output.is_some());
    }
    let recovered = f
        .journal
        .recover(&record.invocation, record.attempt)
        .unwrap();
    let retained = recovered.result.expect("validated result must be durable");
    assert_eq!(retained.digest, result.result_digest);
    assert_eq!(retained.reply.body, result.reply.body);
    // Reusing this invocation must consume the retained result, never send a
    // second POST. This local replay refusal is not a public payment assertion.
    let replay = if streaming {
        f.executor
            .execute_stream(&record.invocation, &bytes, &cancellation, |_| async {
                Ok(())
            })
            .await
    } else {
        f.executor
            .execute_json(&record.invocation, &bytes, &cancellation)
            .await
    };
    assert!(matches!(replay, Err(Error::RecoveryRequired)));
    let rate = result.reply.reported_usage.as_ref().and_then(|u| {
        let duration = last_output?.checked_sub(first_output?)?.as_secs_f64();
        (duration > 0.0 && u.output_tokens > 1).then(|| (u.output_tokens - 1) as f64 / duration)
    });
    let report = json!({"schema_version":1,"endpoint":input.endpoint,"streaming":streaming,
        "inference_invocations":1,"settlement":"local_fixture_only","mainnet_writes":0,
        "completion_ms":completion.as_secs_f64()*1000.0,
        "first_output_ms":first_output.map(|d|d.as_secs_f64()*1000.0),
        "reported_output_tokens":result.reply.reported_usage.as_ref().map(|u|u.output_tokens),
        "approximate_reported_generation_tok_s":rate,"stream_events":events,
        "observed_usage":result.reply.observed_usage,"retained_result_verified":true,
        "replay_refused_without_redispatch":true});
    if let Some(path) = std::env::var_os("MAYHEM_PROXY_LIVE_REPORT") {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        writeln!(file, "{}", serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
    println!("{report}");
}
