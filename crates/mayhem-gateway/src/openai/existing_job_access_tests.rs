use super::*;
use tower::ServiceExt;

fn token(name: &str, models: Vec<String>, revoked: bool) -> GatewayTokenRecord {
    GatewayTokenRecord {
        name: name.into(),
        token_hash: gateway_token_hash(name),
        token_id: format!("tok_{name}"),
        created_at: 1,
        expires_at: None,
        budget_au: Some(10),
        budget_period: Some(GatewayTokenBudgetPeriod::Total),
        spent_total_au: 10,
        spent_period_au: 10,
        period_started_at: Some(1),
        max_rate_per_minute: None,
        models,
        last_used_at: None,
        revoked_at: revoked.then_some(2),
    }
}

async fn get(app: &Router, uri: &str, bearer: &str, key: Option<&str>) -> Response {
    let mut request = axum::http::Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn exhausted_key_can_retrieve_its_paid_job_but_cannot_start_more_inference() {
    let state =
        GatewayState::from_models(Vec::new()).with_access_control(GatewayAccessControl::new(
            true,
            GatewayTokenStore {
                version: 1,
                tokens: vec![token("owner", vec![], false), token("other", vec![], false)],
            },
            None,
        ));
    let family = mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS;
    let id = gateway_job_id(
        state.receipt_config.user_seed,
        Some("tok_owner"),
        family,
        Some("kept-result"),
    )
    .unwrap();
    {
        let mut jobs = state.jobs.lock().unwrap();
        jobs.begin(
            id.clone(),
            family.into(),
            "owned-model".into(),
            Some("tok_owner".into()),
            "fingerprint".into(),
            now_secs(),
        )
        .unwrap();
        jobs.complete(
            &id,
            GatewayJobStatus::Completed,
            Some(json!({"answer":"already paid"})),
            vec![],
            None,
            None,
            now_secs(),
        )
        .unwrap();
    }
    let app = openai_router(state.clone());
    for uri in [
        format!("/v1/jobs/{id}"),
        format!("/v1/jobs/{id}/result"),
        "/v1/jobs".into(),
    ] {
        assert_eq!(
            get(&app, &uri, "owner", None).await.status(),
            StatusCode::OK,
            "{uri}"
        );
    }
    let lookup = format!("/v1/jobs/lookup?endpoint_family={family}");
    assert_eq!(
        get(&app, &lookup, "owner", Some("kept-result"))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        get(&app, &format!("/v1/jobs/{id}/result"), "other", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer owner")
                .body(Body::from(
                    json!({"model":"owned-model","messages":[{"role":"user","content":"again"}]})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert_eq!(
        state.receipt_count(),
        0,
        "result reads must not charge or execute"
    );
}

#[tokio::test]
async fn result_reads_still_enforce_current_revocation_and_model_scope() {
    for (revoked, models, expected) in [
        (true, vec![], StatusCode::UNAUTHORIZED),
        (false, vec!["different-model".into()], StatusCode::FORBIDDEN),
    ] {
        let state =
            GatewayState::from_models(Vec::new()).with_access_control(GatewayAccessControl::new(
                true,
                GatewayTokenStore {
                    version: 1,
                    tokens: vec![token("owner", models, revoked)],
                },
                None,
            ));
        let id = gateway_job_id(
            state.receipt_config.user_seed,
            Some("tok_owner"),
            mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            Some("kept-result"),
        )
        .unwrap();
        {
            let mut jobs = state.jobs.lock().unwrap();
            jobs.begin(
                id.clone(),
                mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS.into(),
                "owned-model".into(),
                Some("tok_owner".into()),
                "fingerprint".into(),
                now_secs(),
            )
            .unwrap();
            jobs.complete(
                &id,
                GatewayJobStatus::Completed,
                Some(json!({"answer":"private"})),
                vec![],
                None,
                None,
                now_secs(),
            )
            .unwrap();
        }
        assert_eq!(
            get(
                &openai_router(state),
                &format!("/v1/jobs/{id}/result"),
                "owner",
                None
            )
            .await
            .status(),
            expected
        );
    }
}
