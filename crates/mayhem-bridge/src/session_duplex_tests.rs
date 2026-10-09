use super::*;
use tokio::net::TcpListener;

type Socket = WebSocketStream<TcpStream>;
fn remote() -> String {
    "a".repeat(64)
}
fn event(n: usize) -> Value {
    json!({"type":"session_frame","remote":remote(),"session_id":"duplex-test", "frame":{"n":n},"direct":true,"relayed":false})
}
async fn read(socket: &mut Socket) -> Value {
    let frame = timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}
async fn send(socket: &mut Socket, value: Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}
async fn ack(socket: &mut Socket, request: &Value) {
    send(socket, json!({"id":request["id"],"type":if request["type"]=="session_close" {"session_closed"} else {"session_sent"},
        "remote":remote(),"session_id":"duplex-test"})).await;
}
async fn connection(count: usize, bytes: usize, pending: Vec<Value>) -> (ScBridgeClient, Socket) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ScBridgeConfig::new(
        format!("ws://{}", listener.local_addr().unwrap()),
        "duplex-test-token",
    )
    .unwrap()
    .with_operation_deadline(Some(Duration::from_secs(2)))
    .with_queue_limits(count, bytes);
    let accept = async {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let auth = read(&mut socket).await;
        assert_eq!(auth["token"], "duplex-test-token");
        send(&mut socket, json!({"id":auth["id"],"type":"auth_ok"})).await;
        socket
    };
    let (client, mut socket) = tokio::join!(ScBridgeClient::connect(config), accept);
    let mut client = client.unwrap();
    let subscribe = async {
        let request = read(&mut socket).await;
        assert_eq!(request["type"], "session_subscribe");
        for value in pending {
            send(&mut socket, value).await;
        }
        send(
            &mut socket,
            json!({"id":request["id"],"type":"session_subscribed"}),
        )
        .await;
    };
    let (result, ()) = tokio::join!(client.session_subscribe(["duplex-test"]), subscribe);
    result.unwrap();
    (client, socket)
}

#[tokio::test]
async fn incoming_event_is_delivered_before_the_send_ack_on_one_authenticated_socket() {
    let (client, mut socket) = connection(4, 8192, vec![]).await;
    let (mut tx, mut rx) = client
        .into_session_duplex(&remote(), "duplex-test")
        .unwrap();
    let (observed, observation) = oneshot::channel();
    let peer = async {
        let request = read(&mut socket).await;
        assert_eq!(request["frame"]["out"], 1);
        send(&mut socket, event(1)).await;
        timeout(Duration::from_secs(1), observation)
            .await
            .unwrap()
            .unwrap();
        ack(&mut socket, &request).await;
        request["id"].as_u64().unwrap()
    };
    let recv = async {
        assert_eq!(rx.next_event().await.unwrap()["frame"]["n"], 1);
        observed.send(()).unwrap();
    };
    let (sent, id, ()) = tokio::join!(tx.send(json!({"out":1})), peer, recv);
    sent.unwrap();
    let close = async {
        let request = read(&mut socket).await;
        assert_eq!(request["id"], id + 1);
        send(
            &mut socket,
            json!({"type":"session_closed","remote":remote(),"session_id":"duplex-test"}),
        )
        .await;
        ack(&mut socket, &request).await;
    };
    let (result, ()) = tokio::join!(tx.close(), close);
    result.unwrap();
    assert_eq!(rx.next_event().await.unwrap()["type"], "session_closed");
}

#[tokio::test]
async fn handoff_preserves_already_queued_events_and_release_reclaims_byte_capacity() {
    let size = json_value_bytes(&event(1));
    let (client, mut socket) = connection(1, size, vec![event(1)]).await;
    let (_tx, mut rx) = client
        .into_session_duplex(&remote(), "duplex-test")
        .unwrap();
    assert_eq!(rx.next_event().await.unwrap()["frame"]["n"], 1);
    send(&mut socket, event(2)).await;
    assert_eq!(rx.next_event().await.unwrap()["frame"]["n"], 2);
}

#[tokio::test]
async fn unconsumed_events_fail_closed_at_both_count_and_byte_bounds() {
    for (count, bytes) in [(1, 8192), (8, json_value_bytes(&event(1)))] {
        let (client, mut socket) = connection(count, bytes, vec![]).await;
        let (_tx, mut rx) = client
            .into_session_duplex(&remote(), "duplex-test")
            .unwrap();
        send(&mut socket, event(1)).await;
        send(&mut socket, event(2)).await;
        timeout(Duration::from_secs(1), rx.ended.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rx.next_event().await.unwrap()["frame"]["n"], 1);
        assert!(matches!(
            rx.next_event().await,
            Err(BridgeError::ResourceLimit(_))
        ));
    }
}

#[tokio::test]
async fn wrong_scope_and_reply_identity_fail_closed_without_leaking_remote_content() {
    for failure in ["remote", "session", "id", "reply_remote", "reply_session"] {
        let (client, mut socket) = connection(4, 8192, vec![]).await;
        let (mut tx, _rx) = client
            .into_session_duplex(&remote(), "duplex-test")
            .unwrap();
        let peer = async {
            let request = read(&mut socket).await;
            let mut value = if failure == "remote" || failure == "session" {
                event(1)
            } else {
                json!({"type":"session_sent","id":request["id"],"remote":remote(),"session_id":"duplex-test"})
            };
            match failure {
                "remote" | "reply_remote" => value["remote"] = json!("c".repeat(64)),
                "session" | "reply_session" => value["session_id"] = json!("other-session"),
                _ => value["id"] = json!(999),
            }
            send(&mut socket, value).await;
        };
        let (result, ()) = tokio::join!(tx.send(json!({"out":1})), peer);
        assert!(matches!(result, Err(BridgeError::Protocol(_))));
        assert!(matches!(
            tx.send(json!({"out":2})).await,
            Err(BridgeError::Closed)
        ));
    }
}

#[tokio::test]
async fn cancelled_send_cannot_reuse_ambiguous_sequence_but_cancelled_event_wait_is_safe() {
    let (client, mut socket) = connection(4, 8192, vec![]).await;
    let (mut tx, mut rx) = client
        .into_session_duplex(&remote(), "duplex-test")
        .unwrap();
    assert!(timeout(Duration::from_millis(10), rx.next_event())
        .await
        .is_err());
    let cancel = async { timeout(Duration::from_millis(60), tx.send(json!({"out":1}))).await };
    let (cancelled, request) = tokio::join!(cancel, read(&mut socket));
    assert!(cancelled.is_err());
    ack(&mut socket, &request).await;
    send(&mut socket, event(1)).await;
    assert_eq!(rx.next_event().await.unwrap()["frame"]["n"], 1);
    assert!(matches!(
        tx.send(json!({"out":2})).await,
        Err(BridgeError::Closed)
    ));
}

#[tokio::test]
async fn dropping_either_half_closes_its_only_socket() {
    for drop_sender in [true, false] {
        let (client, mut socket) = connection(4, 8192, vec![]).await;
        let (tx, rx) = client
            .into_session_duplex(&remote(), "duplex-test")
            .unwrap();
        let mut tx = Some(tx);
        let mut rx = Some(rx);
        if drop_sender {
            drop(tx.take())
        } else {
            drop(rx.take())
        }
        let closed = timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap();
        assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
        if let Some(mut rx) = rx {
            assert!(rx.next_event().await.is_err());
        }
        if let Some(mut tx) = tx {
            assert!(tx.send(json!({"out":1})).await.is_err());
        }
    }
}

#[tokio::test]
async fn deadline_limits_an_unacknowledged_rpc_not_idle_inference_time() {
    let (mut client, mut socket) = connection(4, 8192, vec![]).await;
    client.operation_deadline = Some(Duration::from_millis(40));
    let (mut tx, mut rx) = client
        .into_session_duplex(&remote(), "duplex-test")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(90)).await;
    send(&mut socket, event(1)).await;
    assert_eq!(rx.next_event().await.unwrap()["frame"]["n"], 1);
    let (result, _) = tokio::join!(tx.send(json!({"out":1})), read(&mut socket));
    assert!(matches!(result, Err(BridgeError::Timeout)));
    assert!(matches!(rx.next_event().await, Err(BridgeError::Timeout)));
}

#[tokio::test]
async fn outbound_size_bound_rejects_before_writing_request() {
    let (mut client, mut socket) = connection(4, 8192, vec![]).await;
    client.max_message_bytes = 512;
    let (mut tx, _rx) = client
        .into_session_duplex(&remote(), "duplex-test")
        .unwrap();
    assert!(matches!(
        tx.send(json!({"out":"x".repeat(1024)})).await,
        Err(BridgeError::ResourceLimit(_))
    ));
    let closed = timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
}
