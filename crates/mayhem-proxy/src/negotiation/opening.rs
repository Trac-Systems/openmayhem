//! Opt-in opening handshake over an authenticated literal-loopback SC-Bridge.
//! The dispatcher never owns a model session or sends its readiness response.
use super::*;
use crate::exchange::channel::{bounded_json, validate_config};
use mayhem_bridge::{sc_bridge_session_transport, ScBridgeClient};
use std::time::Instant;

const BOUND: usize = 40 * 1024;
const OPEN: &str = "p.negotiate.open";
#[derive(Serialize, Deserialize)]
#[serde(tag = "t", deny_unknown_fields)]
enum Frame {
    #[serde(rename = "p.negotiate.open")]
    Open {
        schema_version: u32,
        context: Context,
    },
    #[serde(rename = "p.negotiate.ready")]
    Ready {
        schema_version: u32,
        session_id: Digest,
        context_digest: Digest,
    },
}

/// Created only after authenticated bridge peer/session/lineage validation.
/// It cannot be deserialized or cloned into repeated connection attempts.
pub struct Incoming {
    context: Context,
    local: Identity,
    config: ScBridgeConfig,
    limits: exchange::Limits,
    received: Instant,
}
impl Incoming {
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub(crate) fn check(&self, local: &Identity) -> Result<()> {
        if local != &self.local
            || self.received.elapsed() >= self.config.operation_deadline.ok_or(Error::Protocol)?
        {
            return Err(Error::Identity);
        }
        self.context.link(local, Role::Provider)?;
        Ok(())
    }
    /// Only the admitted serving owner may consume this and answer Ready.
    pub(crate) async fn accept(self, local: &Identity) -> Result<Channel> {
        self.check(local)?;
        let mut channel = Channel::connect(
            self.config,
            self.context,
            local,
            Role::Provider,
            self.limits,
        )
        .await?;
        let frame = Frame::Ready {
            schema_version: 1,
            session_id: channel.context.session_id.clone(),
            context_digest: channel.context.digest()?,
        };
        channel
            .wire
            .opening_send(serde_json::to_value(frame).map_err(|_| Error::Protocol)?)
            .await?;
        Ok(channel)
    }
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct Counts {
    pub accepted: u64,
    pub rejected: u64,
    pub unrelated: u64,
}

/// One receiving owner; no context backlog, spawned job, model POST or signature.
/// Other/native session frame tags are ignored. Bridge buffers remain bounded.
pub struct Listener {
    bridge: ScBridgeClient,
    config: ScBridgeConfig,
    local: Identity,
    limits: exchange::Limits,
    counts: Counts,
}
impl Listener {
    pub async fn connect(
        config: ScBridgeConfig,
        local: Identity,
        limits: exchange::Limits,
    ) -> Result<Self> {
        validate_config(&config, limits)?;
        local.validate().map_err(|_| Error::Identity)?;
        let mut bridge = ScBridgeClient::connect(config.clone())
            .await
            .map_err(Error::Transport)?;
        bridge
            .session_subscribe_all()
            .await
            .map_err(Error::Transport)?;
        Ok(Self {
            bridge,
            config,
            local,
            limits,
            counts: Counts::default(),
        })
    }
    pub fn counts(&self) -> Counts {
        self.counts
    }
    pub(crate) fn identity(&self) -> &Identity {
        &self.local
    }
    /// A finite observation wait, not a model timeout. Malformed/foreign opens
    /// cannot renew it or terminate unrelated serving sessions.
    pub async fn next(&mut self, wait: Duration) -> Result<Incoming> {
        let start = Instant::now();
        loop {
            let left = wait
                .checked_sub(start.elapsed())
                .filter(|v| !v.is_zero())
                .ok_or(Error::Interrupted)?;
            let event = self
                .bridge
                .next_session_frame(left)
                .await
                .map_err(Error::Transport)?;
            if event
                .get("frame")
                .and_then(|v| v.get("t"))
                .and_then(Value::as_str)
                != Some(OPEN)
            {
                self.counts.unrelated = self.counts.unrelated.saturating_add(1);
                continue;
            }
            match context_from_event(&event, &self.local) {
                Ok(context) => {
                    self.counts.accepted = self.counts.accepted.saturating_add(1);
                    return Ok(Incoming {
                        context,
                        local: self.local.clone(),
                        config: self.config.clone(),
                        limits: self.limits,
                        received: Instant::now(),
                    });
                }
                Err(_) => self.counts.rejected = self.counts.rejected.saturating_add(1),
            }
        }
    }
}
fn context_from_event(event: &Value, local: &Identity) -> Result<Context> {
    if event["type"] != "session_frame" || sc_bridge_session_transport(event).is_err() {
        return Err(Error::Identity);
    }
    let bytes = bounded_json(event.get("frame").ok_or(Error::Protocol)?, BOUND)?;
    let Frame::Open {
        schema_version: 1,
        context,
    } = serde_json::from_slice(&bytes).map_err(|_| Error::Protocol)?
    else {
        return Err(Error::Protocol);
    };
    context.link(local, Role::Provider)?;
    if event["remote"] != context.buyer.as_str()
        || event["session_id"] != context.session_id.as_str()
    {
        return Err(Error::Identity);
    }
    Ok(context)
}

impl Channel {
    /// The buyer opens once and waits for the actual provider session owner
    /// before sending Request/Recover. Failed opening never publishes a hold.
    pub async fn dial(
        config: ScBridgeConfig,
        context: Context,
        local: &Identity,
        limits: exchange::Limits,
    ) -> Result<Self> {
        let mut channel = Self::connect(config, context, local, Role::Buyer, limits).await?;
        let frame = Frame::Open {
            schema_version: 1,
            context: channel.context.clone(),
        };
        channel
            .wire
            .opening_send(serde_json::to_value(frame).map_err(|_| Error::Protocol)?)
            .await?;
        let frame = channel
            .wire
            .opening_receive(channel.control_deadline)
            .await?;
        let Frame::Ready {
            schema_version: 1,
            session_id,
            context_digest,
        } = serde_json::from_slice(&bounded_json(&frame, BOUND)?).map_err(|_| Error::Protocol)?
        else {
            return Err(Error::Protocol);
        };
        if session_id != channel.context.session_id || context_digest != channel.context.digest()? {
            return Err(Error::Identity);
        }
        Ok(channel)
    }
}
