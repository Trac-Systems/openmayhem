//! Single-purchase bounded provisional output. Closing a receiver is cancellation,
//! never proof of non-execution, financial closure, or permission to retry Execute.
use std::sync::Arc;
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, Debug)]
pub struct StreamLimits {
    pub queued_events: usize,
    pub queued_bytes: usize,
    pub event_bytes: usize,
    pub total_events: usize,
    pub total_bytes: usize,
}
impl StreamLimits {
    /// An observer that accepts every event fitting the owner's declared response
    /// bound. Even the smallest encoded event consumes at least one byte, so
    /// this event-count bound cannot truncate an otherwise byte-valid stream.
    /// Queue bytes are charged separately against the controller buffer budget.
    pub fn for_response_bytes(bytes: usize) -> Self {
        Self {
            queued_events: 8,
            queued_bytes: bytes,
            event_bytes: bytes,
            total_events: bytes,
            total_bytes: bytes,
        }
    }
}
/// Encoded normalized JSON, not SSE framing. No terminal event is emitted here;
/// consumers await the controller's verified outcome before reporting completion.
pub struct StreamEvent {
    json: Vec<u8>,
    _permit: OwnedSemaphorePermit,
    // Queue allocation remains charged even after its controller task ends.
    _accounting: Option<Arc<OwnedSemaphorePermit>>,
}
impl StreamEvent {
    pub fn json_bytes(&self) -> &[u8] {
        &self.json
    }
}
pub struct StreamReceiver {
    receive: mpsc::Receiver<StreamEvent>,
}
impl StreamReceiver {
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        self.receive.recv().await
    }
}
/// Not Clone: one controller operation owns the sender and its accounting limits.
pub struct StreamSender {
    send: mpsc::Sender<StreamEvent>,
    bytes: Arc<Semaphore>,
    pub(super) limits: StreamLimits,
    events: usize,
    total: usize,
    accounting: Option<Arc<OwnedSemaphorePermit>>,
}
pub fn stream_channel(limits: StreamLimits) -> crate::Result<(StreamSender, StreamReceiver)> {
    crate::require(
        (1..=1024).contains(&limits.queued_events)
            && (1..=256 * 1024 * 1024).contains(&limits.total_bytes)
            && (1..=limits.total_bytes).contains(&limits.queued_bytes)
            && (1..=limits.queued_bytes).contains(&limits.event_bytes)
            // JSON cannot encode a zero-byte event: the aggregate byte cap
            // also bounds event count, even if the explicit cap is very large.
            && limits.total_events > 0,
        "invalid buyer stream bounds",
    )?;
    let (send, receive) = mpsc::channel(limits.queued_events);
    Ok((
        StreamSender {
            send,
            bytes: Arc::new(Semaphore::new(limits.queued_bytes)),
            limits,
            events: 0,
            total: 0,
            accounting: None,
        },
        StreamReceiver { receive },
    ))
}
impl StreamSender {
    pub(super) fn would_wait(&self, bytes: usize) -> bool {
        self.bytes.available_permits() < bytes || self.send.capacity() == 0
    }
    pub(super) fn charge(&mut self, permit: OwnedSemaphorePermit) {
        self.accounting = Some(Arc::new(permit));
    }
    pub(super) fn disconnected(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let sender = self.send.clone();
        async move { sender.closed().await }
    }
    pub(super) fn encode(&mut self, value: &serde_json::Value) -> crate::Result<Vec<u8>> {
        let bytes = crate::exchange::channel::bounded_json(value, self.limits.event_bytes)
            .map_err(|_| crate::invalid("buyer stream event exceeds bound"))?;
        self.events = self
            .events
            .checked_add(1)
            .ok_or_else(|| crate::invalid("buyer stream count overflow"))?;
        self.total = self
            .total
            .checked_add(bytes.len())
            .ok_or_else(|| crate::invalid("buyer stream bytes overflow"))?;
        crate::require(
            self.events <= self.limits.total_events && self.total <= self.limits.total_bytes,
            "buyer stream exceeds bound",
        )?;
        Ok(bytes)
    }
    pub(super) async fn send(&self, json: Vec<u8>) -> Result<(), ()> {
        let permit = tokio::select! {
            permit = self.bytes.clone().acquire_many_owned(json.len() as u32) => permit.map_err(|_| ())?,
            _ = self.send.closed() => return Err(()),
        };
        self.send
            .send(StreamEvent {
                json,
                _permit: permit,
                _accounting: self.accounting.clone(),
            })
            .await
            .map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    fn limits() -> StreamLimits {
        StreamLimits {
            queued_events: 1,
            queued_bytes: 32,
            event_bytes: 32,
            total_events: 3,
            total_bytes: 64,
        }
    }
    #[tokio::test]
    async fn queued_memory_keeps_controller_accounting_after_sender_finishes() {
        let budget = Arc::new(Semaphore::new(1));
        let (mut send, mut receive) = stream_channel(limits()).unwrap();
        send.charge(budget.clone().try_acquire_owned().unwrap());
        let encoded = send.encode(&json!("part")).unwrap();
        send.send(encoded).await.unwrap();
        drop(send);
        assert_eq!(budget.available_permits(), 0);
        let event = receive.recv().await.unwrap();
        drop(receive);
        assert_eq!(budget.available_permits(), 0);
        drop(event);
        assert_eq!(budget.available_permits(), 1);
    }
    #[tokio::test]
    async fn queued_event_bytes_remain_reserved_until_consumer_drops_event() {
        let (mut send, mut receive) = stream_channel(limits()).unwrap();
        let a = send
            .encode(&json!("123456789012345678901234567890"))
            .unwrap();
        send.send(a).await.unwrap();
        let event = receive.recv().await.unwrap();
        let b = send.encode(&json!("b")).unwrap();
        let mut pending = Box::pin(send.send(b));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut pending)
                .await
                .is_err()
        );
        drop(event);
        pending.await.unwrap();
        drop(receive);
        assert!(send.send(vec![1]).await.is_err());
    }
    #[test]
    fn independent_event_size_count_total_and_configuration_limits_fail_closed() {
        let (mut send, _) = stream_channel(limits()).unwrap();
        assert!(send.encode(&json!("x".repeat(33))).is_err());
        let (mut send, _) = stream_channel(limits()).unwrap();
        for _ in 0..3 {
            send.encode(&json!(0)).unwrap();
        }
        assert!(send.encode(&json!(0)).is_err());
        let (mut send, _) = stream_channel(limits()).unwrap();
        for _ in 0..2 {
            send.encode(&json!("x".repeat(30))).unwrap();
        }
        assert!(send.encode(&json!(0)).is_err());
        let mut invalid = limits();
        invalid.queued_bytes = usize::MAX;
        assert!(stream_channel(invalid).is_err());
    }
    #[tokio::test]
    async fn declared_response_bound_accepts_more_than_16384_verified_events_without_history() {
        use crate::{
            endpoint::{stream::Stream, Adapter, Limits, PublicAdapter},
            worker::Decoded,
        };
        use mayhem_proto::proxy::ProxyEndpoint;
        use serde_json::Value;
        const COUNT: usize = 20_000;
        const BOUND: usize = 8 * 1024 * 1024;
        let adapter = Adapter::new(
            ProxyEndpoint::Chat,
            mayhem_proto::endpoint_family_contract_template(
                mayhem_proto::ENDPOINT_OPENAI_CHAT_COMPLETIONS,
            )
            .unwrap(),
            "upstream".into(),
            Limits {
                request_bytes: 4096,
                response_bytes: BOUND,
                choices: 1,
                tools: 1,
                questions: 1,
                decision_options: 1,
            },
        )
        .unwrap();
        let bytes = serde_json::to_vec(
            &json!({"model":"public","messages":[{"role":"user","content":"hi"}],"stream":true}),
        )
        .unwrap();
        let provider_request = adapter.prepare_stream(&bytes).unwrap();
        let buyer_request = PublicAdapter::restore(adapter.public_snapshot())
            .unwrap()
            .prepare_stream(&bytes)
            .unwrap();
        let mut provider = Stream::new(&provider_request, "proxy_fixture", 7).unwrap();
        let mut buyer = buyer_request.stream("proxy_fixture", 7).unwrap();
        let (mut sender, mut receiver) =
            stream_channel(StreamLimits::for_response_bytes(BOUND)).unwrap();
        let raw = json!({"id":"upstream","choices":[{"index":0,"delta":{"role":"assistant","content":"x"},"finish_reason":null}]}).to_string();
        for _ in 0..COUNT {
            let event = provider
                .push(Decoded::Sse {
                    event: String::new(),
                    data: raw.clone(),
                    id: None,
                })
                .unwrap()
                .unwrap();
            buyer.push(&event).unwrap();
            let encoded = sender.encode(&event).unwrap();
            sender.send(encoded).await.unwrap();
            let received = receiver.recv().await.unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(received.json_bytes()).unwrap(),
                event
            );
        }
        for value in [
            json!({"id":"upstream","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})
                .to_string(),
            "[DONE]".into(),
        ] {
            assert!(provider
                .push(Decoded::Sse {
                    event: String::new(),
                    data: value,
                    id: None
                })
                .unwrap()
                .is_none());
        }
        let result = provider_request
            .decode_json(provider.finish().unwrap(), "proxy_fixture", 7)
            .unwrap()
            .body;
        buyer.verify_final(&result).unwrap();
        assert_eq!(
            result["choices"][0]["message"]["content"]
                .as_str()
                .unwrap()
                .len(),
            COUNT
        );
        assert_eq!(sender.events, COUNT);
        assert!(sender.total < BOUND);
        assert_eq!(sender.bytes.available_permits(), BOUND);
        drop(sender);
        assert!(receiver.recv().await.is_none());
    }
}
