#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use url::Url;

pub const DEFAULT_SC_BRIDGE_URL: &str = "ws://127.0.0.1:49222";
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:49223/v1";

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type WsRead = futures_util::stream::SplitStream<WsStream>;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("SC-Bridge closed")]
    Closed,
    #[error("timed out waiting for SC-Bridge message")]
    Timeout,
    #[error("SC-Bridge protocol error: {0}")]
    Protocol(String),
    #[error("RPC returned {status}: {body}")]
    RpcStatus { status: StatusCode, body: String },
}

pub type Result<T> = std::result::Result<T, BridgeError>;

#[derive(Debug, Clone)]
pub struct ScBridgeConfig {
    pub url: Url,
    pub token: String,
}

impl ScBridgeConfig {
    pub fn new(url: impl AsRef<str>, token: impl Into<String>) -> Result<Self> {
        Ok(Self {
            url: Url::parse(url.as_ref())?,
            token: token.into(),
        })
    }
}

pub struct ScBridgeClient {
    write: WsSink,
    read: WsRead,
    next_id: u64,
    queued_events: VecDeque<Value>,
}

impl ScBridgeClient {
    pub async fn connect(config: ScBridgeConfig) -> Result<Self> {
        let (stream, _) = connect_async(config.url.as_str()).await?;
        let (write, read) = stream.split();
        let mut client = Self {
            write,
            read,
            next_id: 1,
            queued_events: VecDeque::new(),
        };
        client
            .request(json!({ "type": "auth", "token": config.token }), "auth_ok")
            .await?;
        Ok(client)
    }

    pub async fn ping(&mut self) -> Result<Value> {
        self.request(json!({ "type": "ping" }), "pong").await
    }

    pub async fn subscribe(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Value> {
        let channels = channels
            .into_iter()
            .map(|channel| channel.as_ref().to_owned())
            .collect::<Vec<_>>();
        self.request(
            json!({ "type": "subscribe", "channels": channels }),
            "subscribed",
        )
        .await
    }

    pub async fn join(&mut self, channel: impl AsRef<str>) -> Result<Value> {
        self.request(
            json!({ "type": "join", "channel": channel.as_ref() }),
            "joined",
        )
        .await
    }

    pub async fn open(&mut self, channel: impl AsRef<str>, via: Option<&str>) -> Result<Value> {
        let mut request = json!({ "type": "open", "channel": channel.as_ref() });
        if let Some(via) = via {
            request["via"] = json!(via);
        }
        self.request(request, "open_requested").await
    }

    pub async fn send(
        &mut self,
        channel: impl AsRef<str>,
        message: impl Serialize,
    ) -> Result<Value> {
        self.request(
            json!({
                "type": "send",
                "channel": channel.as_ref(),
                "message": serde_json::to_value(message)?,
            }),
            "sent",
        )
        .await
    }

    pub async fn stats(&mut self) -> Result<Value> {
        self.request(json!({ "type": "stats" }), "stats").await
    }

    pub async fn info(&mut self) -> Result<Value> {
        self.request(json!({ "type": "info" }), "info").await
    }

    pub async fn next_sidechannel_message(&mut self, wait: Duration) -> Result<Value> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(event);
        }

        timeout(wait, async {
            loop {
                let message = self.read_json().await?;
                if message.get("type").and_then(Value::as_str) == Some("sidechannel_message") {
                    return Ok(message);
                }
            }
        })
        .await
        .map_err(|_| BridgeError::Timeout)?
    }

    async fn request(&mut self, mut request: Value, expected_type: &str) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        request["id"] = json!(id);
        self.write
            .send(Message::Text(request.to_string().into()))
            .await?;

        loop {
            let message = self.read_json().await?;
            let message_id = message.get("id").and_then(Value::as_u64);
            if message.get("type").and_then(Value::as_str) == Some("sidechannel_message") {
                self.queued_events.push_back(message);
                continue;
            }
            if message_id != Some(id) {
                continue;
            }
            if message.get("type").and_then(Value::as_str) == Some("error") {
                let error = message
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown SC-Bridge error");
                return Err(BridgeError::Protocol(error.to_owned()));
            }
            let actual_type = message.get("type").and_then(Value::as_str).unwrap_or("");
            if actual_type != expected_type {
                return Err(BridgeError::Protocol(format!(
                    "expected {expected_type}, got {actual_type}"
                )));
            }
            return Ok(message);
        }
    }

    async fn read_json(&mut self) -> Result<Value> {
        while let Some(message) = self.read.next().await {
            match message? {
                Message::Text(text) => return Ok(serde_json::from_str(text.as_ref())?),
                Message::Binary(bytes) => return Ok(serde_json::from_slice(&bytes)?),
                Message::Ping(_) | Message::Pong(_) => continue,
                Message::Close(_) => return Err(BridgeError::Closed),
                Message::Frame(_) => continue,
            }
        }
        Err(BridgeError::Closed)
    }
}

#[derive(Clone)]
pub struct PeerRpcClient {
    base_url: Url,
    http: reqwest::Client,
}

impl PeerRpcClient {
    pub fn new(base_url: impl AsRef<str>) -> Result<Self> {
        let mut base = base_url.as_ref().to_owned();
        if !base.ends_with('/') {
            base.push('/');
        }
        Ok(Self {
            base_url: Url::parse(&base)?,
            http: reqwest::Client::new(),
        })
    }

    pub async fn health(&self) -> Result<Value> {
        self.get("health").await
    }

    pub async fn status(&self) -> Result<Value> {
        self.get("status").await
    }

    pub async fn state(&self, key: Option<&str>, confirmed: Option<bool>) -> Result<Value> {
        let mut url = self.endpoint("state")?;
        if let Some(key) = key {
            url.query_pairs_mut().append_pair("key", key);
        }
        if let Some(confirmed) = confirmed {
            url.query_pairs_mut()
                .append_pair("confirmed", if confirmed { "true" } else { "false" });
        }
        self.request_json(self.http.get(url)).await
    }

    pub async fn state_prefix(
        &self,
        prefix: &str,
        confirmed: Option<bool>,
        limit: Option<u64>,
    ) -> Result<Value> {
        let mut url = self.endpoint("state")?;
        url.query_pairs_mut().append_pair("prefix", prefix);
        if let Some(confirmed) = confirmed {
            url.query_pairs_mut()
                .append_pair("confirmed", if confirmed { "true" } else { "false" });
        }
        if let Some(limit) = limit {
            url.query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        self.request_json(self.http.get(url)).await
    }

    pub async fn contract_schema(&self) -> Result<Value> {
        self.get("contract/schema").await
    }

    pub async fn contract_nonce(&self) -> Result<Value> {
        self.get("contract/nonce").await
    }

    pub async fn prepare_tx(&self, body: impl Serialize) -> Result<Value> {
        self.post("contract/tx/prepare", body).await
    }

    pub async fn submit_tx(&self, body: impl Serialize) -> Result<Value> {
        self.post("contract/tx", body).await
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        let url = self.endpoint(path)?;
        self.request_json(self.http.get(url)).await
    }

    pub async fn post(&self, path: &str, body: impl Serialize) -> Result<Value> {
        let url = self.endpoint(path)?;
        self.request_json(self.http.post(url).json(&body)).await
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        Ok(self.base_url.join(path.trim_start_matches('/'))?)
    }

    async fn request_json(&self, request: reqwest::RequestBuilder) -> Result<Value> {
        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(BridgeError::RpcStatus { status, body });
        }
        Ok(serde_json::from_str(&body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_endpoint_join_preserves_v1_base() {
        let client = PeerRpcClient::new(DEFAULT_RPC_URL).unwrap();
        assert_eq!(
            client.endpoint("contract/schema").unwrap().as_str(),
            "http://127.0.0.1:49223/v1/contract/schema"
        );
    }

    #[test]
    fn rpc_state_prefix_url_uses_state_endpoint() {
        let client = PeerRpcClient::new(DEFAULT_RPC_URL).unwrap();
        let mut url = client.endpoint("state").unwrap();
        url.query_pairs_mut()
            .append_pair("prefix", "enclave/")
            .append_pair("confirmed", "false")
            .append_pair("limit", "10");
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:49223/v1/state?prefix=enclave%2F&confirmed=false&limit=10"
        );
    }

    #[test]
    fn sc_bridge_config_parses_default_url() {
        let config = ScBridgeConfig::new(DEFAULT_SC_BRIDGE_URL, "token").unwrap();
        assert_eq!(config.url.scheme(), "ws");
        assert_eq!(config.url.host_str(), Some("127.0.0.1"));
        assert_eq!(config.url.port(), Some(49222));
        assert_eq!(config.token, "token");
    }
}
