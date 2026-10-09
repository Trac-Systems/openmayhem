use super::*;
use axum::body::to_bytes;
use mayhem_proxy::{
    endpoint::{stream::Stream, Adapter, Limits},
    worker::Decoded,
};

#[path = "../../../../../mayhem-proxy/tests/support/responses.rs"]
mod fixture;

#[tokio::test]
async fn responses_success_tail_matches_normalized_text_reasoning_tools_and_incomplete_protocol() {
    for incomplete in [false, true] {
        let adapter = Adapter::new(
            ProxyEndpoint::Responses,
            mayhem_proto::endpoint_family_contract_template(
                mayhem_proto::ENDPOINT_OPENAI_RESPONSES,
            )
            .unwrap(),
            "upstream".into(),
            Limits {
                request_bytes: 64 * 1024,
                response_bytes: 64 * 1024,
                choices: 8,
                tools: 8,
                questions: 8,
                decision_options: 8,
            },
        )
        .unwrap();
        let bytes = fixture::body();
        let request = adapter.prepare_stream(&bytes).unwrap();
        let mut raw = fixture::flow(r#"{"city":"Paris"}"#);
        if incomplete {
            let last = raw.last_mut().unwrap();
            last["type"] = json!("response.incomplete");
            last["response"]["status"] = json!("incomplete");
            last["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
        }
        let mut provider = Stream::new(&request, "proxy_fixture", 7).unwrap();
        let mut provisional = vec![];
        for value in raw {
            if let Some(event) = provider.push(frame(&value)).unwrap() {
                provisional.push(event);
            }
        }
        let result = request
            .decode_json(provider.finish().unwrap(), "proxy_fixture", 7)
            .unwrap()
            .body;
        let frames = FinalEvents::new(
            ProxyEndpoint::Responses,
            result.clone(),
            provisional.len() as u64,
        )
        .map(Ok::<_, Infallible>);
        let response = Sse::new(stream::iter(frames)).into_response();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let mut verifier = Stream::new(&request, "proxy_fixture", 7).unwrap();
        for event in &provisional {
            verifier.push(frame(event)).unwrap();
        }
        let mut sequence = provisional.len();
        for line in std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
        {
            let event: Value = serde_json::from_str(line).unwrap();
            assert_eq!(event["sequence_number"], sequence);
            sequence += 1;
            verifier.push(frame(&event)).unwrap();
        }
        assert!(verifier.is_done());
        let verified = request
            .decode_json(verifier.finish().unwrap(), "proxy_fixture", 7)
            .unwrap()
            .body;
        assert_eq!(verified, result);
    }
}
fn frame(value: &Value) -> Decoded {
    Decoded::Sse {
        event: String::new(),
        data: value.to_string(),
        id: None,
    }
}
