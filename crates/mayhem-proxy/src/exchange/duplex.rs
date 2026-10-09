use super::{
    channel::{bounded_json, Assembly, Frame, Link, Wire, CHUNK},
    *,
};
use mayhem_bridge::{ScSessionReceiver, ScSessionSender};
use std::time::Duration;

pub struct Sender {
    bridge: ScSessionSender,
    link: Link,
    limits: Limits,
    sent: u64,
    interrupted: bool,
}
pub struct Receiver {
    bridge: ScSessionReceiver,
    link: Link,
    limits: Limits,
    received: u64,
    interrupted: bool,
    session: Session,
}
pub(super) fn split(wire: Wire, session: Session) -> Result<(Sender, Receiver)> {
    if wire.interrupted {
        return Err(Error::Interrupted);
    }
    let (send, recv) = wire
        .bridge
        .into_session_duplex(wire.link.remote.as_str(), &wire.link.session_id)
        .map_err(Error::Transport)?;
    Ok((
        Sender {
            bridge: send,
            link: wire.link.clone(),
            limits: wire.limits,
            sent: wire.sent,
            interrupted: false,
        },
        Receiver {
            bridge: recv,
            link: wire.link,
            limits: wire.limits,
            received: wire.received,
            interrupted: false,
            session,
        },
    ))
}
impl Sender {
    pub async fn send(&mut self, message: &Message) -> Result<()> {
        if self.interrupted {
            return Err(Error::Interrupted);
        }
        if message.sender() != self.link.role {
            return Err(Error::Identity);
        }
        message.validate()?;
        let bytes = bounded_json(message, self.limits.max_message_bytes)?;
        let sequence = self
            .sent
            .checked_add(1)
            .filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Protocol)?;
        let digest = Digest::hash(self.link.purpose.domain(), &[&bytes]);
        self.interrupted = true;
        for offset in (0..bytes.len()).step_by(CHUNK) {
            let frame = Frame::chunk(&self.link, sequence, &bytes, &digest, offset);
            self.bridge
                .send(serde_json::to_value(frame).map_err(|_| Error::Protocol)?)
                .await
                .map_err(Error::Transport)?;
        }
        self.sent = sequence;
        self.interrupted = false;
        Ok(())
    }
    pub async fn close(mut self) -> Result<()> {
        self.bridge.close().await.map_err(Error::Transport)?;
        Ok(())
    }
}
impl Receiver {
    pub fn session(&self) -> &Session {
        &self.session
    }
    /// Keep this future alive while handling outgoing work. Cancelling mid-frame
    /// invalidates this receiver; a fresh connection recovers the same request.
    pub async fn receive(&mut self, wait: Option<Duration>) -> Result<Received> {
        if self.interrupted {
            return Err(Error::Interrupted);
        }
        self.interrupted = true;
        let sequence = self
            .received
            .checked_add(1)
            .filter(|v| *v <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)
            .ok_or(Error::Protocol)?;
        let mut assembly = Assembly::new(sequence);
        let read = async {
            loop {
                let event = self.bridge.next_event().await.map_err(Error::Transport)?;
                if let Some(message) = assembly.push(&self.link, self.limits, event)? {
                    return Ok::<Message, Error>(message);
                }
            }
        };
        let message = match wait {
            Some(wait) => tokio::time::timeout(wait, read)
                .await
                .map_err(|_| Error::Interrupted)??,
            None => read.await?,
        };
        let received = self.session.received(message)?;
        self.received = sequence;
        self.interrupted = false;
        Ok(received)
    }
}
