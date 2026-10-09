//! Opt-in duplex ownership of one already authenticated, subscribed session.
//! One socket/owner, one outstanding RPC, bounded event count and bytes. Native
//! clients retain the existing synchronous request API unless explicitly moved.
use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::AbortHandle,
};

#[cfg(test)]
#[path = "session_duplex_tests.rs"]
mod tests;

#[derive(Clone, Copy)]
enum End {
    Closed,
    Timeout,
    Protocol,
    Full,
}
impl End {
    fn error(self) -> BridgeError {
        match self {
            Self::Closed => BridgeError::Closed,
            Self::Timeout => BridgeError::Timeout,
            Self::Protocol => BridgeError::Protocol("duplex session transport failed".into()),
            Self::Full => BridgeError::ResourceLimit("duplex session event bound exceeded".into()),
        }
    }
}
struct Event {
    value: Option<Value>,
    bytes: usize,
    used: Arc<AtomicUsize>,
}
impl Drop for Event {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Command {
    frame: Option<Value>,
    reply: oneshot::Sender<Result<Value>>,
}
struct Pending {
    id: u64,
    expected: &'static str,
    reply: oneshot::Sender<Result<Value>>,
    deadline: tokio::time::Instant,
}
struct Owner(AbortHandle);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Exclusive sending half. Dropping either half closes this transport only;
/// no remote inference cancellation or financial closure is inferred.
pub struct ScSessionSender {
    commands: mpsc::Sender<Command>,
    ended: watch::Receiver<Option<End>>,
    owner: Arc<Owner>,
    interrupted: bool,
}
pub struct ScSessionReceiver {
    events: mpsc::Receiver<Event>,
    ended: watch::Receiver<Option<End>>,
    owner: Arc<Owner>,
}
impl Drop for ScSessionSender {
    fn drop(&mut self) {
        self.owner.0.abort();
    }
}
impl Drop for ScSessionReceiver {
    fn drop(&mut self) {
        self.owner.0.abort();
    }
}
fn ended(value: &watch::Receiver<Option<End>>) -> BridgeError {
    value.borrow().unwrap_or(End::Closed).error()
}
impl ScSessionSender {
    async fn request(&mut self, frame: Option<Value>) -> Result<Value> {
        if self.interrupted {
            return Err(BridgeError::Closed);
        }
        if self.ended.borrow().is_some() {
            return Err(ended(&self.ended));
        }
        self.interrupted = true;
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command { frame, reply })
            .await
            .map_err(|_| ended(&self.ended))?;
        let value = result.await.map_err(|_| ended(&self.ended))??;
        self.interrupted = false;
        Ok(value)
    }
    pub async fn send(&mut self, frame: Value) -> Result<Value> {
        self.request(Some(frame)).await
    }
    pub async fn close(&mut self) -> Result<Value> {
        self.request(None).await
    }
}
impl ScSessionReceiver {
    /// Cancellation-safe event wait. Logical message assembly belongs to the
    /// caller and must not discard a partial message when selecting other work.
    pub async fn next_event(&mut self) -> Result<Value> {
        let mut event = self.events.recv().await.ok_or_else(|| ended(&self.ended))?;
        Ok(event.value.take().expect("owned event"))
    }
}
fn queue(
    events: &mpsc::Sender<Event>,
    used: &Arc<AtomicUsize>,
    max_bytes: usize,
    value: Value,
    session: &str,
    remote: &str,
) -> std::result::Result<(), End> {
    if !is_session_event(&value) || value["session_id"] != session || value["remote"] != remote {
        return Err(End::Protocol);
    }
    let bytes = json_value_bytes(&value);
    used.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
        n.checked_add(bytes).filter(|v| *v <= max_bytes)
    })
    .map_err(|_| End::Full)?;
    let event = Event {
        value: Some(value),
        bytes,
        used: used.clone(),
    };
    events.try_send(event).map_err(|_| End::Full)
}
impl ScBridgeClient {
    /// Move the existing authenticated socket, subscriptions, pending events and
    /// request sequence. Never opens a second session or transfers its ownership.
    /// Call only between completed request operations; session subscription must
    /// already be established. Only send/close and matching session events remain.
    pub fn into_session_duplex(
        mut self,
        remote: &str,
        session: &str,
    ) -> Result<(ScSessionSender, ScSessionReceiver)> {
        let valid_hex = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        if !valid_hex(remote)
            || session.is_empty()
            || session.len() > 256
            || self.operation_deadline.is_none_or(|d| d.is_zero())
            || self.max_queued_events == 0
            || self.max_queued_bytes == 0
        {
            return Err(BridgeError::Protocol(
                "invalid duplex session configuration".into(),
            ));
        }
        let (commands, command_rx) = mpsc::channel(1);
        let (events, event_rx) = mpsc::channel(self.max_queued_events);
        let (terminal, ended) = watch::channel(None);
        let used = Arc::new(AtomicUsize::new(0));
        for value in std::mem::take(&mut self.queued_events) {
            queue(
                &events,
                &used,
                self.max_queued_bytes,
                value,
                session,
                remote,
            )
            .map_err(End::error)?;
        }
        let task = tokio::spawn(run(
            self,
            remote.into(),
            session.into(),
            command_rx,
            events,
            used,
            terminal,
        ));
        let owner = Arc::new(Owner(task.abort_handle()));
        // The two halves own lifetime via AbortHandle. Dropping this JoinHandle
        // detaches observation, not ownership or an unbounded background process.
        drop(task);
        Ok((
            ScSessionSender {
                commands,
                ended: ended.clone(),
                owner: owner.clone(),
                interrupted: false,
            },
            ScSessionReceiver {
                events: event_rx,
                ended,
                owner,
            },
        ))
    }
}
async fn run(
    client: ScBridgeClient,
    remote: String,
    session: String,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    used: Arc<AtomicUsize>,
    terminal: watch::Sender<Option<End>>,
) {
    let ScBridgeClient {
        mut write,
        mut read,
        mut next_id,
        operation_deadline,
        max_message_bytes,
        max_queued_bytes,
        ..
    } = client;
    let wait = operation_deadline.expect("checked deadline");
    let mut pending: Option<Pending> = None;
    let cause = loop {
        let due = pending.as_ref().map(|p| p.deadline);
        tokio::select! {
            _=async { match due { Some(at)=>tokio::time::sleep_until(at).await, None=>std::future::pending().await } }=>break End::Timeout,
            command=commands.recv(), if pending.is_none()=>{
                let Some(command)=command else {break End::Closed};
                let id=next_id;
                let Some(next)=next_id.checked_add(1).filter(|n|*n<=9_007_199_254_740_991) else {break End::Protocol};
                next_id=next;
                let (request,expected)=match command.frame {
                    Some(frame)=>(json!({"type":"session_send","id":id,"remote":remote,"session_id":session,"frame":frame}),"session_sent"),
                    None=>(json!({"type":"session_close","id":id,"remote":remote,"session_id":session}),"session_closed"),
                };
                let deadline=tokio::time::Instant::now()+wait;
                pending=Some(Pending{id,expected,reply:command.reply,deadline});
                if json_value_bytes(&request)>max_message_bytes {break End::Full};
                match tokio::time::timeout_at(deadline,write.send(Message::Text(request.to_string().into()))).await {
                    Ok(Ok(()))=>(), Ok(Err(_))=>break End::Closed, Err(_)=>break End::Timeout,
                }
            }
            message=read.next()=>{
                let value=match message {
                    Some(Ok(Message::Text(s)))=>serde_json::from_str(&s),
                    Some(Ok(Message::Binary(s)))=>serde_json::from_slice(&s),
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)))=>continue,
                    _=>break End::Closed,
                };
                let Ok(value)=value else {break End::Protocol};
                if is_async_event(&value) {
                    if let Err(e)=queue(&events,&used,max_queued_bytes,value,&session,&remote) {break e};
                    continue;
                }
                let Some(p)=pending.as_ref() else {break End::Protocol};
                if value["id"].as_u64()!=Some(p.id) || value["type"]!=p.expected
                    || value["remote"]!=remote || value["session_id"]!=session {break End::Protocol};
                let p=pending.take().expect("pending request");
                let _=p.reply.send(Ok(value));
            }
        }
    };
    terminal.send_replace(Some(cause));
    if let Some(p) = pending {
        let _ = p.reply.send(Err(cause.error()));
    }
}
