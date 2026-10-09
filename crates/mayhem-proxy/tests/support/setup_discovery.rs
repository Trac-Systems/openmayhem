//! Loopback trusted-peer DOUBLE. No canonical signature/admin mutation is claimed.
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[derive(Clone)]
pub struct Control {
    pub network: Value,
    pub sequence: Arc<AtomicU64>,
    pub fault: Arc<Mutex<Option<&'static str>>>,
    pub queries: Arc<Mutex<Vec<Value>>>,
    pub models_calls: Arc<AtomicU64>,
    pub market: Value,
}
pub struct Server {
    pub url: String,
    pub control: Control,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn start(network: Value) -> Self {
        let endpoint = mayhem_proto::proxy::ProxyEndpoint::Chat;
        let contract = mayhem_proto::endpoint_family_contract_template(
            mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
        )
        .unwrap();
        let market:mayhem_proto::proxy::ProxyMarketDescriptor=serde_json::from_value(json!({"schema_version":1,"lane":"proxy","creator_pubkey":ed25519_dalek::SigningKey::from_bytes(&[201;32]).verifying_key().to_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>(),"slug":"compatible-market","model":{"family_id":"fixture","model_id":"Shared declared model","revision":"","quantization":""},"family":"llm","endpoints":[{"endpoint":endpoint,"contract_hash":mayhem_proto::endpoint_contract_canonical_fingerprint(&contract)}],"metering":mayhem_proxy::metering::Policy::for_endpoint(endpoint).contract(),"pricing":"provider_offers"})).unwrap();
        let control = Control {
            network,
            sequence: Arc::new(AtomicU64::new(0)),
            fault: Arc::new(Mutex::new(None)),
            queries: Arc::new(Mutex::new(vec![])),
            models_calls: Arc::new(AtomicU64::new(0)),
            market: serde_json::to_value(market).unwrap(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let c = control.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let c = c.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut end = None;
                    let mut length = 0;
                    loop {
                        let mut buf = [0u8; 2048];
                        let Ok(n) = stream.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        };
                        bytes.extend_from_slice(&buf[..n]);
                        if bytes.len() > 70_000 {
                            return;
                        };
                        if end.is_none() {
                            if let Some(i) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                                end = Some(i + 4);
                                let headers = String::from_utf8_lossy(&bytes[..i]);
                                length = headers
                                    .lines()
                                    .find_map(|l| {
                                        l.to_ascii_lowercase()
                                            .strip_prefix("content-length:")
                                            .and_then(|n| n.trim().parse::<usize>().ok())
                                    })
                                    .unwrap_or(0);
                            }
                        }
                        if end.is_some_and(|e| bytes.len() >= e + length) {
                            break;
                        }
                    }
                    let line = String::from_utf8_lossy(&bytes[..end.unwrap()]);
                    let (status, value) = if line.starts_with("POST /proxy/discovery ") {
                        let body: Value =
                            serde_json::from_slice(&bytes[end.unwrap()..end.unwrap() + length])
                                .unwrap();
                        c.queries.lock().unwrap().push(body["query"].clone());
                        if *c.fault.lock().unwrap() == Some("unavailable") {
                            (503, json!({"code":"proxy_discovery_unavailable"}))
                        } else {
                            (200, c.page(&body["query"]))
                        }
                    } else if line.starts_with("GET /upstream/models ") {
                        c.models_calls.fetch_add(1, Ordering::SeqCst);
                        (
                            200,
                            json!({"object":"list","data":[{"id":"browser-fixture-model"},{"id":"fixture-model"}]}),
                        )
                    } else {
                        (404, json!({"error":"fixture_route_missing"}))
                    };
                    let body = serde_json::to_vec(&value).unwrap();
                    let header=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                });
            }
        });
        Self { url, control, task }
    }
}
impl Control {
    fn page(&self, q: &Value) -> Value {
        let prefix = "proxy/v1/catalog/";
        let kind = q["kind"].as_str().unwrap();
        let lookup = q["lookup"].as_str();
        let fault = *self.fault.lock().unwrap();
        let mut entries = vec![];
        match kind {
            "families" => {
                let ids = if let Some(id) = lookup {
                    vec![id.to_owned()]
                } else if fault == Some("pages") {
                    let offset = q["cursor"]
                        .as_str()
                        .and_then(|s| s.split('.').nth(1))
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0);
                    (offset..(offset + 40).min(133))
                        .map(|i| format!("family{i:03}"))
                        .collect()
                } else {
                    vec!["fixture".into()]
                };
                for id in ids {
                    if id == "fixture" || id.starts_with("family") {
                        entries.push(json!({"key":format!("{prefix}families/{id}"),"value":{"enabled":fault!=Some("disabled"),"label":format!("Family {id}")}}));
                    }
                }
            }
            "markets" => {
                let market: mayhem_proto::proxy::ProxyMarketDescriptor =
                    serde_json::from_value(self.market.clone()).unwrap();
                let id = market.id().unwrap();
                if lookup.is_none_or(|s| s == id) {
                    entries
                        .push(json!({"key":format!("{prefix}markets/{id}"),"value":self.market}));
                }
            }
            "providers" => {
                let n = self.sequence.load(Ordering::SeqCst);
                if n > 0 {
                    entries.push(json!({"key":format!("{prefix}providers/{}",lookup.unwrap()),"value":{"provider_pubkey":lookup,"admission_id":"08".repeat(32),"sequence":n,"active_memberships":0}}));
                }
            }
            _ => (),
        }
        let next = if kind == "families" && lookup.is_none() && fault == Some("pages") {
            let offset = q["cursor"]
                .as_str()
                .and_then(|s| s.split('.').nth(1))
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
                + 40;
            (offset < 133).then(|| format!("pdc1.{offset}.{}", "00".repeat(64)))
        } else {
            None
        };
        let mut context = self.network.clone();
        context["epoch"] = json!(12);
        let mut binding = q.clone();
        binding.as_object_mut().unwrap().remove("cursor");
        binding.as_object_mut().unwrap().remove("since");
        let mut page = json!({"ok":true,"lane":"proxy","schema_version":1,"request_nonce":"01".repeat(32),"query":binding,"context":context,"proof":{"view_key":"02".repeat(32),"fork":0,"signed_length":17,"tree_hash":"03".repeat(32)},"base_proof":null,"mode":"snapshot","entries":entries,"truncated":next.is_some(),"next_cursor":next,"checkpoint":if next.is_some(){Value::Null}else{json!(format!("pdc1.checkpoint.{}","00".repeat(64)))}});
        match fault {
            Some("identity") => page["context"]["network_id"] = json!("wrong-network"),
            Some("binding") => page["query"]["kind"] = json!("catalog"),
            Some("extra") => page["extra"] = json!(true),
            Some("wrong_entry") => {
                page["entries"] = json!([{"key":"proxy/v1/catalog/families/foreign","value":{"enabled":true,"label":"Foreign"}}])
            }
            Some("duplicate") => {
                let mut e = page["entries"].as_array().unwrap().clone();
                e.extend(e.clone());
                page["entries"] = json!(e);
            }
            _ => (),
        }
        page
    }
}
