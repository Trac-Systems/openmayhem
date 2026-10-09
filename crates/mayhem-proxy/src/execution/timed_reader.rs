//! Separate observation from decoder, fsync and consumer latency, only for streams
//! whose speed is measured. At most eight 64KiB queued pieces and one pending
//! piece; never an unbounded output task. Drop cancels the owned transport task.
use super::*;
use crate::connector::http::UpstreamResponse;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
type Piece = std::result::Result<Option<(Bytes, Instant)>, Failure>;
pub(super) struct Reader {
    direct: Option<UpstreamResponse>,
    measured: Option<(mpsc::Receiver<Piece>, JoinHandle<()>)>,
}
impl Reader {
    pub fn new(mut response: UpstreamResponse, blocked: Option<Arc<AtomicBool>>) -> Self {
        let Some(blocked) = blocked else {
            return Self {
                direct: Some(response),
                measured: None,
            };
        };
        let (tx, rx) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            loop {
                let piece = response.next_timed_chunk().await;
                let terminal = !matches!(piece, Ok(Some(_)));
                // Bytes slices can retain a much larger backing allocation.
                let piece = piece.map(|v| v.map(|(b, at)| (Bytes::copy_from_slice(&b), at)));
                match tx.try_send(piece) {
                    Ok(()) => (),
                    Err(mpsc::error::TrySendError::Full(piece)) => {
                        blocked.store(true, Ordering::Release);
                        if tx.send(piece).await.is_err() {
                            break;
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
                if terminal {
                    break;
                }
            }
        });
        Self {
            direct: None,
            measured: Some((rx, task)),
        }
    }
    pub async fn next(&mut self) -> Result<Option<(Bytes, Instant)>> {
        if let Some(response) = &mut self.direct {
            return response.next_timed_chunk().await.map_err(Error::Upstream);
        }
        let (rx, _) = self.measured.as_mut().ok_or(Error::TransportWorker)?;
        rx.recv()
            .await
            .ok_or(Error::TransportWorker)?
            .map_err(Error::Upstream)
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        if let Some((_, task)) = &self.measured {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::{config::Operation, http::HttpConnection};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn response(pieces: usize, close: bool) -> (UpstreamResponse, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let mut request = Vec::new();
            while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buffer[..n]);
                assert!(request.len() < 8192);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
            for _ in 0..pieces {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                if socket.write_all(b"4\r\ntext\r\n").await.is_err() {
                    return;
                }
            }
            if close {
                socket.write_all(b"0\r\n\r\n").await.unwrap();
            } else {
                assert_eq!(socket.read(&mut buffer).await.unwrap_or(0), 0);
            }
        });
        let connection = HttpConnection::new(
            serde_json::from_value(serde_json::json!({
            "schema_version":1,"id":"fixture","revision":1,"base_url":base,
            "network":{"mode":"pinned","networks":["127.0.0.1/32"],"allow_http":true},
            "paths":{"models":"models"}}))
            .unwrap(),
        )
        .unwrap();
        (
            connection.send(Operation::Models, None).await.unwrap(),
            server,
        )
    }
    #[tokio::test]
    async fn full_read_queue_disqualifies_speed_but_preserves_every_byte_and_terminal_result() {
        let (response, server) = response(12, true).await;
        let blocked = Arc::new(AtomicBool::new(false));
        let mut reader = Reader::new(response, Some(blocked.clone()));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !blocked.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut bytes = Vec::new();
        while let Some((piece, _)) = reader.next().await.unwrap() {
            bytes.extend(piece);
        }
        assert_eq!(bytes, b"text".repeat(12));
        server.await.unwrap();
    }
    #[tokio::test]
    async fn dropping_reader_cancels_its_pending_network_read_and_releases_transport() {
        let (response, server) = response(1, false).await;
        let mut reader = Reader::new(response, Some(Arc::new(AtomicBool::new(false))));
        assert_eq!(reader.next().await.unwrap().unwrap().0, b"text"[..]);
        drop(reader);
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
}
