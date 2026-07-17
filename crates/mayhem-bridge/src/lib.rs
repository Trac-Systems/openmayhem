#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};
use url::Url;

pub const DEFAULT_SC_BRIDGE_URL: &str = "ws://127.0.0.1:49222";
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:49223/v1";
pub const DEFAULT_SC_BRIDGE_MAX_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_SC_BRIDGE_MAX_QUEUED_EVENTS: usize = 4096;
pub const DEFAULT_SC_BRIDGE_MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;

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
    #[error("SC-Bridge resource limit: {0}")]
    ResourceLimit(String),
    #[error("RPC returned {status}: {body}")]
    RpcStatus { status: StatusCode, body: String },
}

pub type Result<T> = std::result::Result<T, BridgeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScBridgeSessionTransport {
    Direct,
    Relayed { relay: String },
}

pub fn sc_bridge_session_transport(value: &Value) -> Result<ScBridgeSessionTransport> {
    let direct = value.get("direct").and_then(Value::as_bool) == Some(true);
    let relayed = value.get("relayed").and_then(Value::as_bool) == Some(true);
    match (direct, relayed) {
        (true, false) => Ok(ScBridgeSessionTransport::Direct),
        (false, true) => {
            let relay = value
                .get("relay")
                .and_then(Value::as_str)
                .filter(|key| {
                    key.len() == 64
                        && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                        && key.bytes().all(|byte| !byte.is_ascii_uppercase())
                })
                .map(str::to_owned)
                .ok_or_else(|| {
                    BridgeError::Protocol(
                        "relayed session must identify a canonical relay peer key".to_owned(),
                    )
                })?;
            if relay.chars().all(|character| character == '0') {
                return Err(BridgeError::Protocol(
                    "relayed session returned the zero relay peer key".to_owned(),
                ));
            }
            Ok(ScBridgeSessionTransport::Relayed { relay })
        }
        _ => Err(BridgeError::Protocol(
            "session must report exactly one of direct or relayed transport".to_owned(),
        )),
    }
}

#[derive(Debug, Clone)]
pub struct ScBridgeConfig {
    pub url: Url,
    pub token: String,
    pub max_message_bytes: usize,
    pub max_queued_events: usize,
    pub max_queued_bytes: usize,
    /// Upper bound on how long one request/reply operation may await its bridge
    /// reply. `None` preserves the historical wait-forever behavior. The bridge is a
    /// loopback socket, so a reply that never arrives means the peer-side handler is
    /// stuck (for example a direct-session send waiting on remote drain); callers that
    /// must stay responsive (the provider session loop) set this so one dead session
    /// cannot silence the whole process. Must be sized above the peer's own bounded
    /// waits (`--session-open-max-wait-ms`, `--session-connect-max-wait-ms`), never
    /// below them.
    pub operation_deadline: Option<Duration>,
}

impl ScBridgeConfig {
    pub fn new(url: impl AsRef<str>, token: impl Into<String>) -> Result<Self> {
        Ok(Self {
            url: Url::parse(url.as_ref())?,
            token: token.into(),
            max_message_bytes: configured_limit(
                "MAYHEM_SC_BRIDGE_MAX_MESSAGE_BYTES",
                DEFAULT_SC_BRIDGE_MAX_MESSAGE_BYTES,
            )?,
            max_queued_events: configured_limit(
                "MAYHEM_SC_BRIDGE_MAX_QUEUED_EVENTS",
                DEFAULT_SC_BRIDGE_MAX_QUEUED_EVENTS,
            )?,
            max_queued_bytes: configured_limit(
                "MAYHEM_SC_BRIDGE_MAX_QUEUED_BYTES",
                DEFAULT_SC_BRIDGE_MAX_QUEUED_BYTES,
            )?,
            operation_deadline: None,
        })
    }

    #[must_use]
    pub fn with_operation_deadline(mut self, deadline: Option<Duration>) -> Self {
        self.operation_deadline = deadline;
        self
    }

    #[must_use]
    pub fn with_queue_limits(mut self, max_events: usize, max_bytes: usize) -> Self {
        self.max_queued_events = max_events.max(1);
        self.max_queued_bytes = max_bytes.max(1);
        self
    }

    #[must_use]
    pub fn with_max_message_bytes(mut self, max_bytes: usize) -> Self {
        self.max_message_bytes = max_bytes.max(1);
        self
    }
}

fn configured_limit(name: &'static str, default: usize) -> Result<usize> {
    let Some(value) = std::env::var(name).ok() else {
        return Ok(default);
    };
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| BridgeError::Protocol(format!("{name} must be a positive integer")))
}

pub struct ScBridgeClient {
    write: WsSink,
    read: WsRead,
    next_id: u64,
    queued_events: VecDeque<Value>,
    queued_event_bytes: usize,
    max_queued_events: usize,
    max_queued_bytes: usize,
    operation_deadline: Option<Duration>,
}

impl ScBridgeClient {
    pub async fn connect(config: ScBridgeConfig) -> Result<Self> {
        let websocket_config = WebSocketConfig::default()
            .max_message_size(Some(config.max_message_bytes))
            .max_frame_size(Some(config.max_message_bytes))
            .max_write_buffer_size(config.max_message_bytes.saturating_mul(2).max(256 * 1024));
        let (stream, _) =
            connect_async_with_config(config.url.as_str(), Some(websocket_config), false).await?;
        let (write, read) = stream.split();
        let mut client = Self {
            write,
            read,
            next_id: 1,
            queued_events: VecDeque::new(),
            queued_event_bytes: 0,
            max_queued_events: config.max_queued_events,
            max_queued_bytes: config.max_queued_bytes,
            operation_deadline: config.operation_deadline,
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

    pub async fn unsubscribe(
        &mut self,
        channels: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Value> {
        let channels = channels
            .into_iter()
            .map(|channel| channel.as_ref().to_owned())
            .collect::<Vec<_>>();
        self.request(
            json!({ "type": "unsubscribe", "channels": channels }),
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

    pub async fn cli(&mut self, command: impl AsRef<str>) -> Result<Value> {
        let response = self
            .request(
                json!({ "type": "cli", "command": command.as_ref() }),
                "cli_result",
            )
            .await?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            let error = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("SC-Bridge CLI command failed");
            return Err(BridgeError::Protocol(error.to_owned()));
        }
        Ok(response)
    }

    pub async fn session_subscribe(
        &mut self,
        session_ids: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Value> {
        let session_ids = session_ids
            .into_iter()
            .map(|session_id| session_id.as_ref().to_owned())
            .collect::<Vec<_>>();
        self.request(
            json!({
                "type": "session_subscribe",
                "session_ids": session_ids,
                "sidechannel": false,
            }),
            "session_subscribed",
        )
        .await
    }

    pub async fn session_subscribe_all(&mut self) -> Result<Value> {
        self.request(
            json!({
                "type": "session_subscribe",
                "session_ids": ["*"],
                "sidechannel": false,
            }),
            "session_subscribed",
        )
        .await
    }

    pub async fn session_open(
        &mut self,
        remote: impl AsRef<str>,
        session_id: impl AsRef<str>,
    ) -> Result<Value> {
        self.request(
            json!({
                "type": "session_open",
                "remote": remote.as_ref(),
                "session_id": session_id.as_ref(),
            }),
            "session_opened",
        )
        .await
    }

    pub async fn peer_connect(&mut self, remote: impl AsRef<str>, wait: Duration) -> Result<Value> {
        let wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
        self.request(
            json!({
                "type": "peer_connect",
                "remote": remote.as_ref(),
                "wait_ms": wait_ms,
            }),
            "peer_connected",
        )
        .await
    }

    pub async fn session_send(
        &mut self,
        remote: impl AsRef<str>,
        session_id: impl AsRef<str>,
        frame: impl Serialize,
    ) -> Result<Value> {
        self.request(
            json!({
                "type": "session_send",
                "remote": remote.as_ref(),
                "session_id": session_id.as_ref(),
                "frame": serde_json::to_value(frame)?,
            }),
            "session_sent",
        )
        .await
    }

    pub async fn session_close(
        &mut self,
        remote: impl AsRef<str>,
        session_id: impl AsRef<str>,
    ) -> Result<Value> {
        self.request(
            json!({
                "type": "session_close",
                "remote": remote.as_ref(),
                "session_id": session_id.as_ref(),
            }),
            "session_closed",
        )
        .await
    }

    pub async fn session_stats(&mut self) -> Result<Value> {
        self.request(json!({ "type": "session_stats" }), "session_stats")
            .await
    }

    pub async fn next_sidechannel_message(&mut self, wait: Duration) -> Result<Value> {
        if let Some(event) = self.pop_queued_event("sidechannel_message") {
            return Ok(event);
        }

        self.next_event_of_type("sidechannel_message", wait).await
    }

    pub async fn next_session_frame(&mut self, wait: Duration) -> Result<Value> {
        if let Some(event) = self.pop_queued_event("session_frame") {
            return Ok(event);
        }

        self.next_event_of_type("session_frame", wait).await
    }

    pub async fn next_session_event(&mut self, wait: Duration) -> Result<Value> {
        if let Some(event) = self.pop_queued_session_event(None) {
            return Ok(event);
        }
        timeout(wait, self.read_session_event(None))
            .await
            .map_err(|_| BridgeError::Timeout)?
    }

    pub async fn next_session_frame_unbounded(&mut self) -> Result<Value> {
        if let Some(event) = self.pop_queued_event("session_frame") {
            return Ok(event);
        }

        self.read_event_of_type("session_frame").await
    }

    pub async fn next_session_frame_for(
        &mut self,
        session_id: impl AsRef<str>,
        wait: Option<Duration>,
    ) -> Result<Value> {
        let session_id = session_id.as_ref();
        if let Some(event) = self.pop_queued_session_frame_event(session_id) {
            return Ok(event);
        }
        let deadline = wait.map(|wait| Instant::now() + wait);
        loop {
            let message = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(BridgeError::Timeout);
                    }
                    timeout(remaining, self.read_json())
                        .await
                        .map_err(|_| BridgeError::Timeout)??
                }
                None => self.read_json().await?,
            };
            if message.get("type").and_then(Value::as_str) == Some("session_frame")
                && message.get("session_id").and_then(Value::as_str) == Some(session_id)
            {
                return Ok(message);
            }
            if is_async_event(&message) {
                self.queue_event_back(message)?;
            }
        }
    }

    pub async fn next_session_event_for(
        &mut self,
        session_id: impl AsRef<str>,
        wait: Option<Duration>,
    ) -> Result<Value> {
        let session_id = session_id.as_ref();
        if let Some(event) = self.pop_queued_session_event(Some(session_id)) {
            return Ok(event);
        }
        match wait {
            Some(wait) => timeout(wait, self.read_session_event(Some(session_id)))
                .await
                .map_err(|_| BridgeError::Timeout)?,
            None => self.read_session_event(Some(session_id)).await,
        }
    }

    pub fn requeue_event_front(&mut self, event: Value) -> Result<()> {
        self.queue_event_front(event)
    }

    async fn next_event_of_type(&mut self, expected_type: &str, wait: Duration) -> Result<Value> {
        timeout(wait, self.read_event_of_type(expected_type))
            .await
            .map_err(|_| BridgeError::Timeout)?
    }

    async fn read_event_of_type(&mut self, expected_type: &str) -> Result<Value> {
        loop {
            let message = self.read_json().await?;
            if message.get("type").and_then(Value::as_str) == Some(expected_type) {
                return Ok(message);
            }
            if is_async_event(&message) {
                self.queue_event_back(message)?;
            }
        }
    }

    fn pop_queued_event(&mut self, expected_type: &str) -> Option<Value> {
        let idx = self
            .queued_events
            .iter()
            .position(|event| event.get("type").and_then(Value::as_str) == Some(expected_type))?;
        let event = self.queued_events.remove(idx)?;
        self.queued_event_bytes = self
            .queued_event_bytes
            .saturating_sub(json_value_bytes(&event));
        Some(event)
    }

    fn pop_queued_session_frame_event(&mut self, session_id: &str) -> Option<Value> {
        let idx = self.queued_events.iter().position(|event| {
            event.get("type").and_then(Value::as_str) == Some("session_frame")
                && event.get("session_id").and_then(Value::as_str) == Some(session_id)
        })?;
        let event = self.queued_events.remove(idx)?;
        self.queued_event_bytes = self
            .queued_event_bytes
            .saturating_sub(json_value_bytes(&event));
        Some(event)
    }

    fn pop_queued_session_event(&mut self, session_id: Option<&str>) -> Option<Value> {
        let idx = self.queued_events.iter().position(|event| {
            is_session_event(event)
                && session_id.is_none_or(|session_id| {
                    event.get("session_id").and_then(Value::as_str) == Some(session_id)
                })
        })?;
        let event = self.queued_events.remove(idx)?;
        self.queued_event_bytes = self
            .queued_event_bytes
            .saturating_sub(json_value_bytes(&event));
        Some(event)
    }

    async fn read_session_event(&mut self, session_id: Option<&str>) -> Result<Value> {
        loop {
            let message = self.read_json().await?;
            if is_session_event(&message)
                && session_id.is_none_or(|session_id| {
                    message.get("session_id").and_then(Value::as_str) == Some(session_id)
                })
            {
                return Ok(message);
            }
            if is_async_event(&message) {
                self.queue_event_back(message)?;
            }
        }
    }

    fn queue_event_back(&mut self, event: Value) -> Result<()> {
        self.ensure_queue_capacity(&event)?;
        self.queued_event_bytes = self
            .queued_event_bytes
            .saturating_add(json_value_bytes(&event));
        self.queued_events.push_back(event);
        Ok(())
    }

    fn queue_event_front(&mut self, event: Value) -> Result<()> {
        self.ensure_queue_capacity(&event)?;
        self.queued_event_bytes = self
            .queued_event_bytes
            .saturating_add(json_value_bytes(&event));
        self.queued_events.push_front(event);
        Ok(())
    }

    fn ensure_queue_capacity(&self, event: &Value) -> Result<()> {
        let event_bytes = json_value_bytes(event);
        if self.queued_events.len() >= self.max_queued_events
            || self.queued_event_bytes.saturating_add(event_bytes) > self.max_queued_bytes
        {
            return Err(BridgeError::ResourceLimit(format!(
                "async event queue exceeds {} events or {} bytes",
                self.max_queued_events, self.max_queued_bytes
            )));
        }
        Ok(())
    }

    async fn request(&mut self, mut request: Value, expected_type: &str) -> Result<Value> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        request["id"] = json!(id);
        let deadline = self.operation_deadline.map(|wait| Instant::now() + wait);
        match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                timeout(
                    remaining,
                    self.write.send(Message::Text(request.to_string().into())),
                )
                .await
                .map_err(|_| BridgeError::Timeout)??;
            }
            None => {
                self.write
                    .send(Message::Text(request.to_string().into()))
                    .await?;
            }
        }

        loop {
            let message = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(BridgeError::Timeout);
                    }
                    timeout(remaining, self.read_json())
                        .await
                        .map_err(|_| BridgeError::Timeout)??
                }
                None => self.read_json().await?,
            };
            let message_id = message.get("id").and_then(Value::as_u64);
            if is_async_event(&message) {
                self.queue_event_back(message)?;
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

fn is_async_event(message: &Value) -> bool {
    match message.get("type").and_then(Value::as_str) {
        // `session_closed` is both the reply type of a `session_close` request AND
        // an async close broadcast. Replies carry the numeric request id; the
        // broadcast never does. Without this distinction every session_close reply
        // was misfiled into the event queue and the caller waited forever
        // (2026-07-13 live wedge: the provider loop stalled per close and the
        // gateway hung after receipt ack, so requests never returned).
        Some("session_closed") => message.get("id").and_then(Value::as_u64).is_none(),
        // Sidechannel events carry the sidechannel payload's own (string) id, and
        // session frames carry none; neither collides with numeric request ids.
        Some("sidechannel_message" | "session_frame") => true,
        _ => false,
    }
}

fn is_session_event(message: &Value) -> bool {
    matches!(
        message.get("type").and_then(Value::as_str),
        Some("session_frame" | "session_closed")
    )
}

fn json_value_bytes(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
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

    pub async fn submit_feature(&self, body: impl Serialize) -> Result<Value> {
        self.post("contract/feature", body).await
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
    fn session_transport_requires_one_authenticated_path() {
        assert_eq!(
            sc_bridge_session_transport(&json!({ "direct": true, "relayed": false })).unwrap(),
            ScBridgeSessionTransport::Direct
        );
        assert_eq!(
            sc_bridge_session_transport(&json!({
                "direct": false,
                "relayed": true,
                "relay": "a".repeat(64)
            }))
            .unwrap(),
            ScBridgeSessionTransport::Relayed {
                relay: "a".repeat(64)
            }
        );
        assert!(sc_bridge_session_transport(&json!({
            "direct": true,
            "relayed": true
        }))
        .is_err());
        assert!(sc_bridge_session_transport(&json!({
            "direct": false,
            "relayed": true
        }))
        .is_err());
        assert!(sc_bridge_session_transport(&json!({
            "direct": false,
            "relayed": true,
            "relay": "A".repeat(64)
        }))
        .is_err());
        assert!(sc_bridge_session_transport(&json!({
            "direct": false,
            "relayed": true,
            "relay": "not-a-key"
        }))
        .is_err());
    }

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
        assert_eq!(
            config.max_message_bytes,
            DEFAULT_SC_BRIDGE_MAX_MESSAGE_BYTES
        );
        assert_eq!(
            config.max_queued_events,
            DEFAULT_SC_BRIDGE_MAX_QUEUED_EVENTS
        );
    }

    #[tokio::test]
    async fn session_close_reply_is_not_swallowed_by_the_close_event_queue() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let session = "dd".repeat(32);
        let server_session = session.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let auth = socket.next().await.unwrap().unwrap();
            let auth: Value = match auth {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => panic!("expected auth text, got {other:?}"),
            };
            socket
                .send(Message::Text(
                    json!({"id": auth["id"], "type": "auth_ok"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let close = socket.next().await.unwrap().unwrap();
            let close: Value = match close {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => panic!("expected close text, got {other:?}"),
            };
            // An async close broadcast (no id) arrives before the actual reply.
            socket
                .send(Message::Text(
                    json!({
                        "type": "session_closed",
                        "session_id": server_session,
                        "reason": "remote closed",
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "id": close["id"],
                        "type": "session_closed",
                        "session_id": server_session,
                        "closed": true,
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let mut client = ScBridgeClient::connect(
            ScBridgeConfig::new(format!("ws://{address}"), "token")
                .unwrap()
                .with_operation_deadline(Some(Duration::from_secs(2))),
        )
        .await
        .unwrap();
        let closed = client
            .session_close("ee".repeat(32), &session)
            .await
            .unwrap();
        assert_eq!(closed["closed"], true);
        // The async broadcast stays available as an event.
        let event = client
            .next_session_event(Duration::from_millis(50))
            .await
            .unwrap();
        assert_eq!(event["type"], "session_closed");
        assert_eq!(event["reason"], "remote closed");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn operation_deadline_unblocks_a_request_whose_reply_never_arrives() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let auth = socket.next().await.unwrap().unwrap();
            let auth: Value = match auth {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => panic!("expected auth text, got {other:?}"),
            };
            socket
                .send(Message::Text(
                    json!({"id": auth["id"], "type": "auth_ok"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            // Read the stats request, deliver an unrelated async event, then go
            // silent: the wedge under test is a bridge handler that never replies.
            let _stats = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "type": "session_frame",
                        "session_id": "cc".repeat(32),
                        "frame": {"t": "s.open"},
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            // Keep the socket open until the client has timed out.
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        let mut client = ScBridgeClient::connect(
            ScBridgeConfig::new(format!("ws://{address}"), "token")
                .unwrap()
                .with_operation_deadline(Some(Duration::from_millis(150))),
        )
        .await
        .unwrap();
        let error = client.stats().await.unwrap_err();
        assert!(matches!(error, BridgeError::Timeout), "got {error:?}");
        // The async event that arrived while awaiting the reply is preserved.
        let queued = client
            .next_session_frame(Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(queued["session_id"], "cc".repeat(32));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn targeted_session_wait_defers_other_session_frames_in_order() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let wanted = "aa".repeat(32);
        let other = "bb".repeat(32);
        let server_wanted = wanted.clone();
        let server_other = other.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let auth = socket.next().await.unwrap().unwrap();
            let auth: Value = match auth {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => panic!("expected auth text, got {other:?}"),
            };
            socket
                .send(Message::Text(
                    json!({"id": auth["id"], "type": "auth_ok"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            for session_id in [server_other, server_wanted] {
                socket
                    .send(Message::Text(
                        json!({
                            "type": "session_frame",
                            "session_id": session_id,
                            "frame": {"t": "s.open"},
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
        });

        let mut client = ScBridgeClient::connect(
            ScBridgeConfig::new(format!("ws://{address}"), "token").unwrap(),
        )
        .await
        .unwrap();
        let selected = client
            .next_session_frame_for(&wanted, Some(Duration::from_secs(1)))
            .await
            .unwrap();
        assert_eq!(selected["session_id"], wanted);
        let deferred = client
            .next_session_frame(Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(deferred["session_id"], other);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn targeted_session_wait_fails_closed_when_unrelated_event_queue_is_full() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let wanted = "aa".repeat(32);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let auth = socket.next().await.unwrap().unwrap();
            let auth: Value = match auth {
                Message::Text(text) => serde_json::from_str(text.as_ref()).unwrap(),
                other => panic!("expected auth text, got {other:?}"),
            };
            socket
                .send(Message::Text(
                    json!({"id": auth["id"], "type": "auth_ok"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            for index in 0..3 {
                socket
                    .send(Message::Text(
                        json!({
                            "type": "session_frame",
                            "session_id": format!("{index:064x}"),
                            "frame": {"t": "s.open"},
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
        });

        let config = ScBridgeConfig::new(format!("ws://{address}"), "token")
            .unwrap()
            .with_queue_limits(2, 4096);
        let mut client = ScBridgeClient::connect(config).await.unwrap();
        let err = client
            .next_session_frame_for(&wanted, Some(Duration::from_secs(1)))
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::ResourceLimit(_)));
        server.await.unwrap();
    }
}
