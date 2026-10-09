use super::tests::{
    long_running_test_child, send_control_request, test_runtime, wait_for_test_state,
};
use super::*;
#[cfg(unix)]
#[tokio::test]
async fn exact_child_inspection_http_auth_lost_ack_and_restart() {
    let temp = env::temp_dir().join(format!(
        "mayhemd-inspect-{}-{}",
        std::process::id(),
        unix_epoch_millis().unwrap()
    ));
    fs::create_dir_all(&temp).unwrap();
    let runtime = test_runtime(&temp, &[]);
    let child = long_running_test_child("provider-proxy-fixture");
    let hash = persistent::child_config_hash(&child).unwrap();
    let token = "test-control-token-0123456789abcdef";
    async fn inspect(
        runtime: &SupervisorRuntime,
        name: &str,
        hash: &str,
        token: Option<&str>,
    ) -> (u16, serde_json::Value) {
        let (tx, _rx) = mpsc::channel(1);
        let body = json!({"name":name,"expected_config_hash":hash}).to_string();
        let auth = token
            .map(|v| format!("Authorization: Bearer {v}\r\n"))
            .unwrap_or_default();
        let wire=format!("POST /children/inspect HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: {}\r\n\r\n{body}",body.len());
        let response = send_control_request(
            runtime.clone(),
            tx,
            Some("test-control-token-0123456789abcdef"),
            &wire,
        )
        .await;
        let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
        let value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        (status, value)
    }
    assert_eq!(inspect(&runtime, &child.name, &hash, None).await.0, 401);
    assert_eq!(
        inspect(&runtime, &child.name, &hash, Some("wrong")).await.0,
        401
    );
    assert_eq!(inspect(&runtime, "../bad", &hash, Some(token)).await.0, 400);
    assert_eq!(
        inspect(&runtime, &child.name, "malformed", Some(token))
            .await
            .0,
        400
    );
    let missing = inspect(&runtime, &child.name, &hash, Some(token)).await.1;
    assert_eq!(missing["state"], "missing");
    assert!(!temp.join("supervisor-private").exists());
    let (control_tx, mut rx) = mpsc::channel(1);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let task = tokio::spawn(handle_status_connection(
        server,
        runtime.clone(),
        control_tx,
        Some(Arc::from(token)),
    ));
    let mut body = serde_json::to_value(&child).unwrap();
    body["persistent"] = json!(true);
    let body = body.to_string();
    client.write_all(format!("POST /children/add HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    let command = rx.recv().await.unwrap();
    drop(client); // ACK cannot reach installer
    let mut tasks = JoinSet::new();
    let mut shutdowns = BTreeMap::new();
    handle_supervisor_command(command, &runtime, &mut tasks, &mut shutdowns).await;
    let _ = task.await.unwrap();
    wait_for_test_state(
        Duration::from_secs(3),
        |v| v.children[&child.name].running,
        &runtime,
    )
    .await;
    let installed = inspect(&runtime, &child.name, &hash, Some(token)).await.1;
    assert_eq!(installed["state"], "matched");
    assert_eq!(installed["persistent"], true);
    assert_eq!(installed["lifecycle"]["running"], true);
    let different = inspect(&runtime, &child.name, &"0".repeat(64), Some(token))
        .await
        .1;
    assert_eq!(different["state"], "mismatch");
    for private in [
        "command",
        "args",
        "env",
        "cwd",
        "startup_probes",
        "last_error",
    ] {
        assert!(!installed.to_string().contains(&format!("\"{private}\"")));
    }
    shutdowns[&child.name].send_replace(true);
    while tasks.join_next().await.is_some() {}
    drop(shutdowns);
    drop(runtime);
    let (store, restored) = persistent::Store::load(&temp).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(persistent::child_config_hash(&restored[0]).unwrap(), hash);
    drop(store);
    let runtime = test_runtime(&temp, &restored);
    let stopped = inspect(&runtime, &child.name, &hash, Some(token)).await.1;
    assert_eq!(stopped["state"], "matched");
    assert_eq!(stopped["lifecycle"]["running"], false);
    let mut shutdowns = BTreeMap::new();
    spawn_supervised_child(restored[0].clone(), &runtime, &mut tasks, &mut shutdowns).unwrap();
    wait_for_test_state(
        Duration::from_secs(3),
        |v| v.children[&child.name].running,
        &runtime,
    )
    .await;
    assert_eq!(
        inspect(&runtime, &child.name, &hash, Some(token)).await.1["state"],
        "matched"
    );
    shutdowns[&child.name].send_replace(true);
    while tasks.join_next().await.is_some() {}
    let native = long_running_test_child("native-existing");
    runtime.add_child_config(&native).await.unwrap();
    assert_eq!(
        inspect(&runtime, &native.name, &hash, Some(token)).await.1["state"],
        "nonpersistent"
    );
    if let Some(path) = env::var_os("MAYHEM_SUPERVISOR_INSPECT_EVIDENCE") {
        fs::write(path,serde_json::to_vec_pretty(&json!({"schema_version":1,"fixture":"actual_authenticated_supervisor_http_durable_restart_lost_ack","missing":missing,"installed":installed,"stopped":stopped,"mismatch":different,"one_restored_child":true})).unwrap()).unwrap();
    }
    drop(shutdowns);
    drop(runtime);
    fs::remove_dir_all(temp).unwrap();
}
