use super::*;
use crate::exchange::{channel::bounded_json, Message};
use tokio::sync::{mpsc, oneshot};

pub(super) struct Entry {
    pub message: Message,
    pub delivered: Option<oneshot::Sender<()>>,
    _bytes: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::PublicState;

    fn message() -> Message {
        Message::State {
            state: PublicState::Running,
        }
    }
    fn queue(count: usize, copies: usize) -> (Output, mpsc::Receiver<Entry>) {
        let bound = serde_json::to_vec(&message()).unwrap().len() * copies;
        let (sender, receiver) = mpsc::channel(count);
        (
            Output {
                sender,
                bytes: Arc::new(Semaphore::new(bound)),
                bound,
            },
            receiver,
        )
    }
    #[tokio::test]
    async fn cancelled_byte_wait_keeps_live_entry_reserved_and_disconnect_unblocks_producer() {
        let (output, mut receiver) = queue(2, 1);
        output.send(message()).await.unwrap();
        let entry = receiver.recv().await.unwrap();
        assert!(matches!(output.control(message()), Err(Error::Busy)));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), output.send(message()))
                .await
                .is_err()
        );
        assert_eq!(output.bytes.available_permits(), 0);
        drop(entry);
        output.send(message()).await.unwrap();
        let held = receiver.recv().await.unwrap();
        let producer = tokio::spawn({
            let output = output.clone();
            async move { output.send(message()).await }
        });
        drop(receiver);
        assert!(tokio::time::timeout(Duration::from_secs(1), producer)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        drop(held);
        assert_eq!(output.bytes.available_permits(), output.bound);
    }
    #[tokio::test]
    async fn delivery_requires_writer_ack_and_count_backpressure_reclaims_bytes() {
        let (output, mut receiver) = queue(1, 4);
        output.send(message()).await.unwrap();
        let left = output.bytes.available_permits();
        assert!(matches!(output.control(message()), Err(Error::Busy)));
        assert_eq!(output.bytes.available_permits(), left);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), output.send(message()))
                .await
                .is_err()
        );
        assert_eq!(output.bytes.available_permits(), left);
        drop(receiver.recv().await.unwrap());
        let mut delivery = tokio::spawn({
            let output = output.clone();
            async move { output.deliver(message()).await }
        });
        let mut entry = receiver.recv().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut delivery)
                .await
                .is_err()
        );
        entry.delivered.take().unwrap().send(()).unwrap();
        delivery.await.unwrap().unwrap();
        drop(entry);
        let delivery = tokio::spawn({
            let output = output.clone();
            async move { output.deliver(message()).await }
        });
        drop(receiver.recv().await.unwrap());
        assert!(delivery.await.unwrap().is_err());
        assert_eq!(output.bytes.available_permits(), output.bound);
    }
}
#[derive(Clone)]
pub(super) struct Output {
    sender: mpsc::Sender<Entry>,
    bytes: Arc<Semaphore>,
    bound: usize,
}
pub(super) fn channel(limits: Limits) -> (Output, mpsc::Receiver<Entry>) {
    let (sender, receiver) = mpsc::channel(limits.outbound_messages);
    (
        Output {
            sender,
            bytes: Arc::new(Semaphore::new(limits.outbound_bytes)),
            bound: limits.outbound_bytes,
        },
        receiver,
    )
}
impl Output {
    fn size(&self, message: &Message) -> Result<u32> {
        let bytes = bounded_json(message, self.bound)?.len();
        u32::try_from(bytes).map_err(|_| Error::Configuration)
    }
    pub async fn send(&self, message: Message) -> Result<()> {
        self.enqueue(message, None).await
    }
    pub async fn deliver(&self, message: Message) -> Result<()> {
        let (sent, received) = oneshot::channel();
        self.enqueue(message, Some(sent)).await?;
        received
            .await
            .map_err(|_| Error::Transport(exchange::Error::Interrupted))
    }
    async fn enqueue(
        &self,
        message: Message,
        delivered: Option<oneshot::Sender<()>>,
    ) -> Result<()> {
        let size = self.size(&message)?;
        let reservation = tokio::select! {
            p=self.bytes.clone().acquire_many_owned(size)=>p.map_err(|_|Error::Task)?,
            _=self.sender.closed()=>return Err(Error::Transport(exchange::Error::Interrupted)),
        };
        self.sender
            .send(Entry {
                message,
                delivered,
                _bytes: reservation,
            })
            .await
            .map_err(|_| Error::Transport(exchange::Error::Interrupted))
    }
    pub fn control(&self, message: Message) -> Result<()> {
        let size = self.size(&message)?;
        let reservation = self
            .bytes
            .clone()
            .try_acquire_many_owned(size)
            .map_err(|_| Error::Busy)?;
        self.sender
            .try_send(Entry {
                message,
                delivered: None,
                _bytes: reservation,
            })
            .map_err(|_| Error::Busy)
    }
}
