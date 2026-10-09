//! Bounded local SC-Bridge protocol double. Authenticated peer attribution is
//! supplied by this test transport; it is NOT a real Noise/relay network proof.
use futures_util::{SinkExt, StreamExt};
use mayhem_bridge::ScBridgeConfig;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{mpsc, Mutex},
    task::{JoinHandle, JoinSet},
};
use tokio_tungstenite::{accept_async, tungstenite::Message};

pub struct Bridge {
    url: String,
    pub attack: Arc<Mutex<Option<&'static str>>>,
    pub frames: Arc<Mutex<Vec<Value>>>,
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
        let mode = attack.clone();
        let capture = frames.clone();
        let identities = Arc::new(HashMap::from([
            ("test-buyer".to_owned(), buyer.to_owned()),
            ("test-provider".to_owned(), provider.to_owned()),
        ]));
        let peers: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let Ok((socket,_))=accepted else {break};
                        let peers=peers.clone();let identities=identities.clone();let mode=mode.clone();let capture=capture.clone();
                        tasks.spawn(async move {
                            let ws=accept_async(socket).await.unwrap();let (mut out,mut input)=ws.split();
                            let first=input.next().await.unwrap().unwrap();
                            let auth:Value=serde_json::from_str(first.to_text().unwrap()).unwrap();
                            let Some(own)=identities.get(auth["token"].as_str().unwrap_or("")).cloned() else {return};
                            let (tx,mut incoming)=mpsc::channel::<Value>(8);
                            peers.lock().await.insert(own.clone(),tx.clone());
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
                                if kind=="session_send" {
                                    let target=peers.lock().await.get(request["remote"].as_str().unwrap()).cloned();
                                    let Some(target)=target else {break};
                                    let mut event=json!({"type":"session_frame","remote":own,"session_id":request["session_id"],
                                        "direct":true,"relayed":false,"frame":request["frame"]});
                                    let attack=mode.lock().await.take();
                                    match attack {
                                        Some("remote")=>event["remote"]=json!("f".repeat(64)),
                                        Some("terms")=>event["frame"]["accepted_terms"]=json!("f".repeat(64)),
                                        Some("offset")=>event["frame"]["offset"]=json!(1),
                                        Some("sequence")=>event["frame"]["sequence"]=json!(2),
                                        Some("size")=>event["frame"]["bytes"]=json!(999_999_999),
                                        Some("digest")=>event["frame"]["digest"]=json!("f".repeat(64)),
                                        Some("extra")=>event["frame"]["unexpected"]=json!(true),
                                        Some("lineage")=>event["direct"]=json!(false),
                                        _=>{},
                                    }
                                    {let mut stored=capture.lock().await;if stored.len()<256{stored.push(event.clone());}}
                                    if target.send(event.clone()).await.is_err(){break}
                                    if attack==Some("duplicate") && target.send(event).await.is_err(){break}
                                }
                                let typ=match kind {"session_subscribe"=>"session_subscribed","session_open"=>"session_opened",
                                    "session_send"=>"session_sent","session_close"=>"session_closed",_=>panic!("unexpected test command")};
                                if out.send(Message::Text(json!({"type":typ,"id":request["id"],"remote":request["remote"],
                                    "session_id":request["session_id"],"direct":true,"relayed":false}).to_string().into())).await.is_err(){break}
                            }
                            let mut peers=peers.lock().await;
                            if peers.get(&own).is_some_and(|old|old.same_channel(&tx)){peers.remove(&own);}
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
