use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use mayhem_bridge::{sc_bridge_session_transport, ScBridgeClient, ScBridgeConfig};
use std::time::{Duration, Instant};

const CHUNK: usize = 32 * 1024;
const FRAME_BOUND: usize = 64 * 1024;
#[derive(Clone, Copy)]
pub struct Limits {
    /// Total serialized logical message bound supplied from admission/resource
    /// configuration. Fragment size is independent of model context size.
    pub max_message_bytes: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    t: String,
    schema_version: u32,
    session_id: String,
    accepted_terms: Digest,
    sequence: u64,
    bytes: usize,
    offset: usize,
    digest: Digest,
    data: String,
}

/// One authenticated direct/relayed session over a protected local Core bridge.
/// An interrupted transfer poisons this channel; reconnect to the same logical
/// request. It never resets finance, dispatch intent or upstream idempotency.
pub struct Channel {
    bridge: ScBridgeClient,
    session: Session,
    limits: Limits,
    sent: u64,
    received: u64,
    interrupted: bool,
}
impl Channel {
    pub async fn connect(config: ScBridgeConfig, session: Session, limits: Limits) -> Result<Self> {
        // The existing local token authenticates a trusted Core process. A
        // remote plaintext bridge must not impersonate Noise peer identities.
        let loopback = match config.url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if !loopback
            || !matches!(config.url.scheme(), "ws" | "wss")
            || config.url.query().is_some()
            || config.url.fragment().is_some()
            || !config.url.username().is_empty()
            || config.url.password().is_some()
            || config.operation_deadline.is_none_or(|d| d.is_zero())
            || config.max_message_bytes < FRAME_BOUND
            || limits.max_message_bytes == 0
            || limits.max_message_bytes > 256 * 1024 * 1024
        {
            return Err(Error::Protocol);
        }
        let mut bridge = ScBridgeClient::connect(config)
            .await
            .map_err(Error::Transport)?;
        let t = &session.authorization.terms;
        bridge
            .session_subscribe([t.session_id.as_str()])
            .await
            .map_err(Error::Transport)?;
        if session.role == Role::Buyer {
            let opened = bridge
                .session_open(session.remote.as_str(), &t.session_id)
                .await
                .map_err(Error::Transport)?;
            if opened["remote"] != session.remote.as_str()
                || opened["session_id"] != t.session_id
                || sc_bridge_session_transport(&opened).is_err()
            {
                return Err(Error::Identity);
            }
        }
        Ok(Self {
            bridge,
            session,
            limits,
            sent: 0,
            received: 0,
            interrupted: false,
        })
    }
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub async fn send(&mut self, message: &Message) -> Result<()> {
        if self.interrupted {
            return Err(Error::Interrupted);
        }
        if message.sender() != self.session.role {
            return Err(Error::Identity);
        }
        message.validate()?;
        let bytes = bounded_json(message, self.limits.max_message_bytes)?;
        let sequence = self
            .sent
            .checked_add(1)
            .filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Protocol)?;
        let digest = Digest::hash("mayhem/proxy/exchange-payload/v1", &[&bytes]);
        self.interrupted = true;
        for (index, chunk) in bytes.chunks(CHUNK).enumerate() {
            let frame = Frame {
                t: "p.exchange".into(),
                schema_version: 1,
                session_id: self.session.authorization.terms.session_id.clone(),
                accepted_terms: self.session.accepted_terms.clone(),
                sequence,
                bytes: bytes.len(),
                offset: index * CHUNK,
                digest: digest.clone(),
                data: STANDARD.encode(chunk),
            };
            self.bridge
                .session_send(self.session.remote.as_str(), &frame.session_id, &frame)
                .await
                .map_err(Error::Transport)?;
        }
        self.sent = sequence;
        self.interrupted = false;
        Ok(())
    }
    /// `wait` bounds this control/message read, not total generation duration.
    /// The same deadline spans all fragments; fragments cannot extend it.
    pub async fn receive(&mut self, wait: Option<Duration>) -> Result<Received> {
        if self.interrupted {
            return Err(Error::Interrupted);
        }
        self.interrupted = true;
        let start = Instant::now();
        let sequence = self
            .received
            .checked_add(1)
            .filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Protocol)?;
        let mut bytes = Vec::new();
        let mut header: Option<(usize, Digest)> = None;
        loop {
            let remaining = match wait {
                Some(wait) => Some(
                    wait.checked_sub(start.elapsed())
                        .filter(|d| !d.is_zero())
                        .ok_or(Error::Interrupted)?,
                ),
                None => None,
            };
            let event = self
                .bridge
                .next_session_event_for(&self.session.authorization.terms.session_id, remaining)
                .await
                .map_err(Error::Transport)?;
            if event["remote"] != self.session.remote.as_str()
                || event["session_id"] != self.session.authorization.terms.session_id
            {
                return Err(Error::Identity);
            }
            match event["type"].as_str() {
                Some("session_opened") => continue,
                Some("session_frame") => {}
                _ => return Err(Error::Interrupted),
            }
            if sc_bridge_session_transport(&event).is_err() {
                return Err(Error::Identity);
            }
            let value = event.get("frame").ok_or(Error::Protocol)?;
            let encoded = bounded_json(value, FRAME_BOUND)?;
            let frame: Frame = serde_json::from_slice(&encoded).map_err(|_| Error::Protocol)?;
            if frame.t != "p.exchange"
                || frame.schema_version != 1
                || frame.sequence != sequence
                || frame.accepted_terms != self.session.accepted_terms
                || frame.session_id != self.session.authorization.terms.session_id
                || frame.bytes == 0
                || frame.bytes > self.limits.max_message_bytes
                || frame.offset != bytes.len()
                || frame.offset >= frame.bytes
            {
                return Err(Error::Protocol);
            }
            if let Some((total, digest)) = &header {
                if *total != frame.bytes || *digest != frame.digest {
                    return Err(Error::Protocol);
                }
            } else {
                header = Some((frame.bytes, frame.digest.clone()));
            }
            if frame.data.len() > CHUNK.div_ceil(3) * 4 {
                return Err(Error::Protocol);
            }
            let part = STANDARD.decode(&frame.data).map_err(|_| Error::Protocol)?;
            if part.len() != CHUNK.min(frame.bytes - frame.offset) {
                return Err(Error::Protocol);
            }
            bytes.extend_from_slice(&part);
            if bytes.len() != frame.bytes {
                continue;
            }
            if Digest::hash("mayhem/proxy/exchange-payload/v1", &[&bytes]) != frame.digest {
                return Err(Error::Protocol);
            }
            let message: Message = serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)?;
            message.validate()?;
            if message.sender() != self.session.role.opposite() {
                return Err(Error::Identity);
            }
            self.received = sequence;
            self.interrupted = false;
            return Ok(Received {
                accepted_terms: self.session.accepted_terms.clone(),
                recipient: self.session.role,
                message,
            });
        }
    }
    pub async fn close(mut self) -> Result<()> {
        self.bridge
            .session_close(
                self.session.remote.as_str(),
                &self.session.authorization.terms.session_id,
            )
            .await
            .map_err(Error::Transport)?;
        Ok(())
    }
}

/// Avoid allocating an arbitrarily large serialization before checking its size.
fn bounded_json(value: &impl Serialize, bound: usize) -> Result<Vec<u8>> {
    struct Writer {
        bytes: Vec<u8>,
        bound: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.bound.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("exchange message bound"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        bound,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| Error::Protocol)?;
    Ok(writer.bytes)
}
