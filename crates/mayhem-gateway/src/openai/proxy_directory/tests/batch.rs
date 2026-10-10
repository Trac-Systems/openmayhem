use super::*;

fn batch_uri(ids: &[String]) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("ids", &ids.join(","))
        .finish();
    format!("/v1/proxy/offers/batch?{query}")
}

#[tokio::test]
async fn batch_binds_exact_ids_and_missing_publications_without_changing_catalog_or_native_models()
{
    let _serial = HTTP_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let control = protected_control(dir.path());
    apply(&control, rows("Batch fixture", "fixture", 2, false));
    let state = state(Some(control.clone()));
    let (_, _, page) = request(&state, "/v1/proxy/offers", Some(TOKEN)).await;
    let first = page["entries"][0]["id"].as_str().unwrap().to_owned();
    let second = page["entries"][1]["id"].as_str().unwrap().to_owned();
    let missing = format!("{}/{}/{}", "f".repeat(64), "e".repeat(64), "d".repeat(64));
    let ids = vec![second.clone(), missing.clone(), first.clone()];
    let before = control.catalog().read().unwrap().status().committed;
    let native_before = native_catalog(request(&state, "/v1/models", Some(TOKEN)).await.2);
    let (status, _, batch) = request(&state, &batch_uri(&ids), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(batch["object"], "proxy.offer_batch");
    assert_eq!(batch["entries"].as_array().unwrap().len(), 3);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(batch["entries"][i]["id"], *id);
        if i == 1 {
            assert!(batch["entries"][i]["offer"].is_null());
        } else {
            let offer = &batch["entries"][i]["offer"];
            assert_eq!(offer["id"], *id);
            let listed = page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["id"] == *id)
                .unwrap();
            assert_eq!(
                offer["availability"]["status"],
                listed["availability"]["status"]
            );
            assert_eq!(
                offer["availability"]["observed_at_ms"],
                batch["observed_at_ms"]
            );
        }
    }
    assert_eq!(control.catalog().read().unwrap().status().committed, before);
    assert_eq!(
        native_catalog(request(&state, "/v1/models", Some(TOKEN)).await.2),
        native_before
    );
    let (status, _, _) = request(&state, &batch_uri(&ids), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn batch_rejects_duplicate_malformed_unknown_and_excessive_queries_before_reads() {
    let _serial = HTTP_TESTS.lock().await;
    let state = state(None);
    let id = format!("{}/{}/{}", "a".repeat(64), "b".repeat(64), "c".repeat(64));
    for path in [
        batch_uri(&[]),
        batch_uri(&[id.clone(), id.clone()]),
        batch_uri(&["invalid".into()]),
        format!("{}&extra=1", batch_uri(&[id.clone()])),
        format!("{}&ids={id}", batch_uri(&[id.clone()])),
        batch_uri(
            &(0..17)
                .map(|n| format!("{n:064x}/{}/{}", "b".repeat(64), "c".repeat(64)))
                .collect::<Vec<_>>(),
        ),
    ] {
        let (status, _, body) = request(&state, &path, Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(error_code(&body), "invalid_request_error");
    }
    let (status, _, body) = request(&state, &batch_uri(&[id]), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "proxy_directory_disabled");
}
