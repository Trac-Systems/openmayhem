use super::*;
use std::sync::atomic::Ordering;
async fn read(c: &reqwest::Client, s: &Server, cookie: &str, csrf: &str, body: Value) -> Value {
    let r = post(c, s, cookie, csrf, "bootstrap/guide")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    let v = r.json::<Value>().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{v}");
    v
}
#[tokio::test]
async fn guided_dashboard_protected_models_catalog_prices_and_join_save() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (cookie, state) = session(&c, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    let peer = s.peer.as_ref().unwrap();
    let url = format!(
        "{}/mayhem/dashboard/provider/setup/bootstrap/guide",
        s.origin
    );
    assert_eq!(
        c.post(&url)
            .body("malformed")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&c, &s, &cookie, "wrong", "bootstrap/guide")
            .body("malformed")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(peer.control.queries.lock().unwrap().is_empty());
    let preview=read(&c,&s,&cookie,csrf,json!({"kind":"models","base_url":format!("{}upstream/",peer.url),"network_policy":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},"credential":{"kind":"bearer_value","value":"synthetic-preview-key"}})).await;
    assert_eq!(preview["preview"]["state"], "listed");
    assert_eq!(preview["preview"]["model_ids"].as_array().unwrap().len(), 2);
    assert_eq!(preview["preview"]["model_identity"], "not_verified");
    assert!(!preview.to_string().contains("synthetic-preview-key"));
    assert_eq!(peer.control.models_calls.load(Ordering::SeqCst), 1);
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|f| f
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".proxy-models-")));
    let families = read(
        &c,
        &s,
        &cookie,
        csrf,
        json!({"kind":"catalog","browse":{"kind":"families","cursor":null}}),
    )
    .await;
    assert_eq!(
        families["result"]["page"]["entries"][0]["value"]["label"],
        "Family fixture"
    );
    let markets=read(&c,&s,&cookie,csrf,json!({"kind":"catalog","browse":{"kind":"markets","family_id":"fixture","endpoint":"openai_chat_completions","cursor":null}})).await;
    assert_eq!(
        markets["result"]["compatible_market_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let prices=read(&c,&s,&cookie,csrf,json!({"kind":"amounts","rates":[{"unit":"input_token","granularity":1000,"usd":"0.000000000000000123"},{"unit":"output_token","granularity":1000,"usd":"0.000000000000000456"}],"per_request_usd":"0.000000000000000002","min_session_usd":"0.000000000000000003","probe_total_usd":"0.00002","probe_per_attempt_usd":"0.00001"})).await;
    assert_eq!(prices["result"]["rates"][0]["per_unit_au"], "123");
    assert_eq!(prices["result"]["max_cost_microusd"], 20);
    let mut chosen = input();
    chosen["market"] = json!({"action":"join_market","market":peer.control.market});
    let r = post(&c, &s, &cookie, csrf, "bootstrap")
        .json(&chosen)
        .send()
        .await
        .unwrap();
    let status = r.status();
    let body = r.json::<Value>().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let saved = FlowConfig::load(&dir.path().join("proxy-setup/wizard.json")).unwrap();
    assert!(matches!(
        saved.profile.market,
        ProfileMarket::JoinMarket { .. }
    ));
    assert_eq!(saved.profile.sequence, 1);
    assert_eq!(saved.profile.offers[0].rates[0].per_unit_au, 123);
    assert!(!dir
        .path()
        .join("proxy-setup/runtime/capacity.redb")
        .exists());
    if let Some(path) = std::env::var_os("MAYHEM_GUIDED_SETUP_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"fixture":"actual authenticated gateway, private factory and upstream GET; canonical peer is a read-only loopback double","models":preview,"families":families,"markets":markets,"prices":prices,"created":body,"no_probe_payment_publication_run":true})).unwrap()).unwrap();
    }
}
#[tokio::test]
async fn guided_dashboard_stale_disabled_foreign_or_unavailable_selection_never_saves() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (cookie, state) = session(&c, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    let peer = s.peer.as_ref().unwrap();
    peer.control.sequence.store(1, Ordering::SeqCst);
    let sequence = read(&c, &s, &cookie, csrf, json!({"kind":"sequence"})).await;
    assert_eq!(sequence["result"]["sequence"], 2);
    let r = post(&c, &s, &cookie, csrf, "bootstrap")
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(!dir.path().join("proxy-setup").exists());
    peer.control.sequence.store(0, Ordering::SeqCst);
    for fault in [
        "disabled",
        "identity",
        "binding",
        "extra",
        "wrong_entry",
        "duplicate",
        "unavailable",
    ] {
        *peer.control.fault.lock().unwrap() = Some(fault);
        let r = post(&c, &s, &cookie, csrf, "bootstrap")
            .json(&input())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::CONFLICT, "{fault}");
        assert!(!dir.path().join("proxy-setup").exists());
    }
    *peer.control.fault.lock().unwrap() = None;
    let mut existing = input();
    existing["market"] = json!({"action":"create_market","slug":peer.control.market["slug"],"model":peer.control.market["model"]});
    let duplicate = post(&c, &s, &cookie, csrf, "bootstrap")
        .json(&existing)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(
        duplicate.json::<Value>().await.unwrap()["error"],
        "setup_market_exists_choose_join"
    );
    let mut altered = input();
    let mut market = peer.control.market.clone();
    market["model"]["model_id"] = json!("Changed identity");
    altered["market"] = json!({"action":"join_market","market":market});
    assert_eq!(
        post(&c, &s, &cookie, csrf, "bootstrap")
            .json(&altered)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert!(!dir.path().join("proxy-setup").exists());
}
#[tokio::test]
async fn guided_dashboard_original_recovery_precedes_unavailable_fresh_reads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (cookie, state) = session(&c, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    let peer = s.peer.as_ref().unwrap();
    let cfg = config(dir.path());
    let choices = cfg
        .choices(serde_json::from_value(input()).unwrap())
        .unwrap();
    bootstrap::create(&cfg.destination, cfg.host, choices).unwrap();
    *peer.control.fault.lock().unwrap() = Some("unavailable");
    let r = post(&c, &s, &cookie, csrf, "bootstrap")
        .json(&input())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.json::<Value>().await.unwrap()["created"], false);
    assert!(peer.control.queries.lock().unwrap().is_empty());
}
#[tokio::test]
async fn guided_dashboard_canonical_pages_reach_deep_families_without_hydration() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (cookie, state) = session(&c, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    let peer = s.peer.as_ref().unwrap();
    *peer.control.fault.lock().unwrap() = Some("pages");
    let mut cursor = Value::Null;
    let mut seen = 0;
    loop {
        let v = read(
            &c,
            &s,
            &cookie,
            csrf,
            json!({"kind":"catalog","browse":{"kind":"families","cursor":cursor}}),
        )
        .await;
        seen += v["result"]["page"]["entries"].as_array().unwrap().len();
        cursor = v["result"]["page"]["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(seen, 133);
    assert_eq!(peer.control.queries.lock().unwrap().len(), 4);
    let bad=post(&c,&s,&cookie,csrf,"bootstrap/guide").json(&json!({"kind":"catalog","browse":{"kind":"markets","family_id":"fixture","endpoint":"openai_chat_completions","cursor":null,"unbounded":true}})).send().await.unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn guided_dashboard_preview_guards_and_excess_precision_fail_before_writes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let s = start(dir.path()).await;
    let c = reqwest::Client::new();
    let (cookie, state) = session(&c, &s).await;
    let csrf = state["csrf"].as_str().unwrap();
    let peer = s.peer.as_ref().unwrap();
    let mut preview = json!({"kind":"models","base_url":format!("{}upstream/",peer.url),"network_policy":{"mode":"public_https"},"credential":{"kind":"none"}});
    assert_eq!(
        post(&c, &s, &cookie, csrf, "bootstrap/guide")
            .json(&preview)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    preview["network_policy"] =
        json!({"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true});
    for credential in [
        json!({"kind":"reference","id":"unapproved"}),
        json!({"kind":"file","path":"/not-an-approved-reference"}),
    ] {
        preview["credential"] = credential;
        assert_eq!(
            post(&c, &s, &cookie, csrf, "bootstrap/guide")
                .json(&preview)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for (price, probe) in [
        ("1e3", "0"),
        ("340282366920938463463.374607431768211456", "0"),
        ("0", "0.0000001"),
    ] {
        let amounts = json!({"kind":"amounts","rates":[{"unit":"decision","granularity":1,"usd":price}],"per_request_usd":"0","min_session_usd":"0","probe_total_usd":probe,"probe_per_attempt_usd":"0"});
        assert_eq!(
            post(&c, &s, &cookie, csrf, "bootstrap/guide")
                .json(&amounts)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(peer.control.models_calls.load(Ordering::SeqCst), 0);
    assert!(peer.control.queries.lock().unwrap().is_empty());
    assert!(!dir.path().join("proxy-setup").exists());
}
