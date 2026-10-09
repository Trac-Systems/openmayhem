use super::*;
use axum::response::{sse::Event, Response, Sse};
use futures_util::stream;
use std::{convert::Infallible, sync::atomic::AtomicBool};
use tokio::sync::Semaphore;

#[path = "../../../../../../mayhem-proxy/tests/support/responses.rs"]
mod responses;

pub(crate) struct StreamBackend {
    pub paused: AtomicBool,
    pub release: Semaphore,
    pub pieces: AtomicUsize,
    pub chunk_bytes: AtomicUsize,
    pub emitted: AtomicUsize,
}
impl Default for StreamBackend {
    fn default() -> Self {
        Self {
            paused: AtomicBool::new(false),
            release: Semaphore::new(0),
            pieces: AtomicUsize::new(1),
            chunk_bytes: AtomicUsize::new(0),
            emitted: AtomicUsize::new(0),
        }
    }
}
pub(super) fn response(endpoint: ProxyEndpoint, control: Arc<StreamBackend>) -> Response {
    let mut values = vec![];
    if endpoint == ProxyEndpoint::Responses {
        values = responses::text_flow("hello", false)
            .into_iter()
            .map(Some)
            .collect();
    } else {
        let count = control.pieces.load(Ordering::SeqCst);
        let large = "x".repeat(control.chunk_bytes.load(Ordering::SeqCst));
        for n in 0..=count {
            let terminal = n == count;
            let text = if terminal {
                ""
            } else if !large.is_empty() {
                large.as_str()
            } else if count == 1 {
                "hello"
            } else {
                "x"
            };
            let choice = if endpoint == ProxyEndpoint::Chat {
                json!({"index":0,"delta":{"role":"assistant","content":text},"finish_reason":terminal.then_some("stop")})
            } else {
                json!({"index":0,"text":text,"finish_reason":terminal.then_some("stop")})
            };
            values.push(Some(json!({"id":"upstream-stream","choices":[choice]})));
        }
        values.push(None);
    }
    let stream = stream::unfold(
        (values.into_iter(), control, 0usize),
        |(mut values, control, count)| async move {
            let value = values.next()?;
            if count == 1 && control.paused.load(Ordering::SeqCst) {
                control.release.acquire().await.unwrap().forget();
            }
            control.emitted.fetch_add(1, Ordering::SeqCst);
            let event = match value {
                Some(value) => Event::default().json_data(value).unwrap(),
                None => Event::default().data("[DONE]"),
            };
            Some((Ok::<_, Infallible>(event), (values, control, count + 1)))
        },
    );
    Sse::new(stream).into_response()
}
