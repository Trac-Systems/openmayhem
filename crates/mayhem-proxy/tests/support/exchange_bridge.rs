//! Bounded local SC-Bridge protocol double. Authenticated peer attribution is
//! supplied by this test transport; it is NOT a real Noise/relay network proof.
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use mayhem_bridge::ScBridgeConfig;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, Mutex},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

struct Client {
    own: String,
    tx: mpsc::Sender<Value>,
    sessions: HashSet<String>,
    all: bool,
    channels: HashSet<String>,
}
pub struct Bridge {
    url: String,
    pub attack: Arc<Mutex<Option<&'static str>>>,
    pub frames: Arc<Mutex<Vec<Value>>>,
    pub reject_presence: Arc<AtomicBool>,
    task: JoinHandle<()>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Bridge {
    pub async fn start(buyer: &str, provider: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let attack = Arc::new(Mutex::new(None));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let reject_presence = Arc::new(AtomicBool::new(false));
        let rejection = reject_presence.clone();
        let mode = attack.clone();
        let capture = frames.clone();
        let identities = Arc::new(HashMap::from([
            ("test-buyer".to_owned(), buyer.to_owned()),
            ("test-provider".to_owned(), provider.to_owned()),
        ]));
        let peers: Arc<Mutex<HashMap<u64, Client>>> = Arc::new(Mutex::new(HashMap::new()));
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let mut client_id = 0u64;
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let Ok((socket,_))=accepted else {break};
                        client_id+=1;let id=client_id;
                        let peers=peers.clone();let identities=identities.clone();let mode=mode.clone();let capture=capture.clone();let rejection=rejection.clone();
                        tasks.spawn(async move {
                            let ws=accept_async(socket).await.unwrap();let (mut out,mut input)=ws.split();
                            let first=input.next().await.unwrap().unwrap();
                            let auth:Value=serde_json::from_str(first.to_text().unwrap()).unwrap();
                            let Some(own)=identities.get(auth["token"].as_str().unwrap_or("")).cloned() else {return};
                            let (tx,mut incoming)=mpsc::channel::<Value>(8);
                            {
                                let mut peers=peers.lock().await;
                                if peers.len()>=64 {return}
                                peers.insert(id,Client{own:own.clone(),tx:tx.clone(),sessions:HashSet::new(),all:false,channels:HashSet::new()});
                            }
                            out.send(Message::Text(json!({"type":"auth_ok","id":auth["id"]}).to_string().into())).await.unwrap();
                            loop {
                                let request=tokio::select! {
                                    v=incoming.recv()=>{
                                        let Some(v)=v else {break};
                                        if out.send(Message::Text(v.to_string().into())).await.is_err(){break}
                                        continue;
                                    },
                                    v=input.next()=>{
                                        let Some(Ok(v))=v else {break};
                                        if v.is_close(){break};
                                        let Ok(text)=v.to_text() else {continue};
                                        serde_json::from_str::<Value>(text).unwrap()
                                    }
                                };
                                let kind=request["type"].as_str().unwrap();
                                if kind=="send" && rejection.load(Ordering::SeqCst) {
                                    if out.send(Message::Text(json!({"type":"error","id":request["id"],"error":"fixture presence transport unavailable"}).to_string().into())).await.is_err(){break}
                                    continue;
                                }
                                if kind=="subscribe" {
                                    let mut peers=peers.lock().await;let client=peers.get_mut(&id).unwrap();
                                    let selected=request["channels"].as_array().unwrap();
                                    if selected.is_empty(){client.channels.clear();}
                                    for channel in selected {client.channels.insert(channel.as_str().unwrap().into());}
                                }
                                if kind=="send" {
                                    let channel=request["channel"].as_str().unwrap();
                                    let targets=peers.lock().await.values().filter(|client|client.channels.contains(channel)).map(|client|client.tx.clone()).collect::<Vec<_>>();
                                    for target in targets {let _=target.try_send(json!({"type":"sidechannel_message","channel":channel,"message":request["message"]}));}
                                }
                                if kind=="session_subscribe" {
                                    let mut peers=peers.lock().await;let client=peers.get_mut(&id).unwrap();
                                    for session in request["session_ids"].as_array().unwrap() {
                                        let session=session.as_str().unwrap();
                                        if session=="*" {client.all=true} else {client.sessions.insert(session.into());}
                                    }
                                }
                                if kind=="session_send" {
                                    let targets=peers.lock().await.values().filter(|client|client.own==request["remote"].as_str().unwrap()
                                        && (client.all || client.sessions.contains(request["session_id"].as_str().unwrap())))
                                        .map(|client|client.tx.clone()).collect::<Vec<_>>();
                                    if targets.is_empty(){break}
                                    let mut event=json!({"type":"session_frame","remote":own,"session_id":request["session_id"],
                                        "direct":true,"relayed":false,"frame":request["frame"]});
                                    let attack={
                                        let mut mode=mode.lock().await;
                                        if matches!(*mode,Some("stream_content"|"stream_identity")) {
                                            let payload=event["frame"]["data"].as_str().and_then(|v|STANDARD.decode(v).ok())
                                                .and_then(|v|serde_json::from_slice::<Value>(&v).ok());
                                            if payload.as_ref().is_some_and(|v|v["kind"]=="stream") {mode.take()} else {None}
                                        } else {mode.take()}
                                    };
                                    match attack {
                                        Some("remote")=>event["remote"]=json!("f".repeat(64)),
                                        Some("terms")=>event["frame"]["accepted_terms"]=json!("f".repeat(64)),
                                        Some("negotiation")=>event["frame"]["negotiation"]=json!("f".repeat(64)),
                                        Some("purpose")=>event["frame"]["t"]=json!("p.exchange"),
                                        Some("stream_content"|"stream_identity")=>{
                                            // Authenticated malicious-provider fixture: recompute
                                            // transport framing so the buyer's event/final verifier,
                                            // not a damaged checksum, must detect this mutation.
                                            let bytes=STANDARD.decode(event["frame"]["data"].as_str().unwrap()).unwrap();
                                            let mut payload:Value=serde_json::from_slice(&bytes).unwrap();
                                            if attack==Some("stream_content") {payload["event"]["choices"][0]["delta"]["content"]=json!("different");}
                                            else {payload["event"]["id"]=json!("foreign-public-id");}
                                            let bytes=serde_json::to_vec(&payload).unwrap();
                                            let mut h=blake3::Hasher::new_derive_key("mayhem/proxy/exchange-payload/v1");
                                            h.update(&(bytes.len() as u64).to_le_bytes());h.update(&bytes);
                                            event["frame"]["bytes"]=json!(bytes.len());
                                            event["frame"]["data"]=json!(STANDARD.encode(&bytes));
                                            event["frame"]["digest"]=json!(h.finalize().to_hex().to_string());
                                        },
                                        Some("request_payload" | "signature_payload")=>{
                                            let bytes=STANDARD.decode(event["frame"]["data"].as_str().unwrap()).unwrap();
                                            let mut payload:Value=serde_json::from_slice(&bytes).unwrap();
                                            if attack==Some("request_payload") { payload["request"]["model"]=json!("altered-model"); }
                                            else { payload["value"]["authorization"]["provider_sig"]=json!("0".repeat(128)); }
                                            let bytes=serde_json::to_vec(&payload).unwrap();
                                            let mut h=blake3::Hasher::new_derive_key("mayhem/proxy/negotiation-payload/v1");
                                            h.update(&(bytes.len() as u64).to_le_bytes());h.update(&bytes);
                                            event["frame"]["bytes"]=json!(bytes.len());
                                            event["frame"]["data"]=json!(STANDARD.encode(&bytes));
                                            event["frame"]["digest"]=json!(h.finalize().to_hex().to_string());
                                        },
                                        Some("offset")=>event["frame"]["offset"]=json!(1),
                                        Some("sequence")=>event["frame"]["sequence"]=json!(2),
                                        Some("size")=>event["frame"]["bytes"]=json!(999_999_999),
                                        Some("digest")=>event["frame"]["digest"]=json!("f".repeat(64)),
                                        Some("extra")=>event["frame"]["unexpected"]=json!(true),
                                        Some("ready_context")=>event["frame"]["context_digest"]=json!("f".repeat(64)),
                                        Some("lineage")=>event["direct"]=json!(false),
                                        _=>{},
                                    }
                                    {let mut stored=capture.lock().await;if stored.len()<256{stored.push(event.clone());}}
                                    for target in targets {
                                        if target.send(event.clone()).await.is_err(){continue}
                                        if attack==Some("duplicate") {let _=target.send(event.clone()).await;}
                                    }
                                    if attack==Some("lost_ack"){break}
                                }
                                if kind=="send" {let mut frames=capture.lock().await;if frames.len()<256 {frames.push(request.clone());}}
                                let typ=match kind {"session_subscribe"=>"session_subscribed","session_open"=>"session_opened",
                                    "session_send"=>"session_sent","session_close"=>"session_closed","join"=>"joined","send"=>"sent","subscribe"=>"subscribed","clear_filter"=>"filter_set",_=>panic!("unexpected test command")};
                                if out.send(Message::Text(json!({"type":typ,"id":request["id"],"remote":request["remote"],
                                    "session_id":request["session_id"],"direct":true,"relayed":false}).to_string().into())).await.is_err(){break}
                            }
                            peers.lock().await.remove(&id);
                        });
                    },
                    Some(_) = tasks.join_next(), if !tasks.is_empty()=>{},
                }
            }
        });
        Self {
            url,
            attack,
            frames,
            reject_presence,
            task,
        }
    }
    pub fn config(&self, buyer: bool) -> ScBridgeConfig {
        ScBridgeConfig::new(
            &self.url,
            if buyer { "test-buyer" } else { "test-provider" },
        )
        .unwrap()
        .with_operation_deadline(Some(Duration::from_secs(5)))
        .with_queue_limits(16, 2 * 1024 * 1024)
    }
}
