//! Shared protected transport and decoder path for paid execution and operator probes.
use super::*;

pub(super) struct Transport<'a> {
    pub connection: &'a HttpConnection,
    pub adapter: &'a Adapter,
}
pub(super) enum Delivery<'a> {
    Customer {
        storage: &'a Storage,
        record: &'a Record,
    },
    Probe {
        created: u64,
    },
}
impl Delivery<'_> {
    fn created(&self) -> u64 {
        match self {
            Self::Customer { record, .. } => record.created_at_ms / 1000,
            Self::Probe { created } => *created,
        }
    }
    async fn first_output(&self) -> Result<()> {
        if let Self::Customer { storage, record } = self {
            storage
                .event(&record.invocation, record.attempt, Event::FirstOutput)
                .await?;
        }
        Ok(())
    }
    async fn upstream_id(&self, id: &str) -> Result<()> {
        if let Self::Customer { storage, record } = self {
            storage
                .event(
                    &record.invocation,
                    record.attempt,
                    Event::Accepted(attempts::RemoteId::new(id)?),
                )
                .await?;
        }
        Ok(())
    }
}
impl Transport<'_> {
    pub(super) async fn perform_stream<F, Fut>(
        &self,
        request: &Request,
        mut decoder: worker::host::Active,
        public_id: &str,
        delivery: &Delivery<'_>,
        emit: &mut F,
        sample: &mut Option<health::Sample>,
    ) -> Result<ProtocolReply>
    where
        F: FnMut(serde_json::Value) -> Fut,
        Fut: Future<Output = std::result::Result<(), ()>>,
    {
        let response = self
            .connection
            .send(self.adapter.operation(), Some(request.body().to_vec()))
            .await
            .map_err(Error::Upstream)?;
        if let Some(sample) = sample {
            sample.headers();
        }
        if response.status != 200 || response.format != WireFormat::Sse {
            return Err(Error::Upstream(Failure::new(
                Code::UpstreamProtocol,
                Scope::Model,
                Stage::ResponseHeaders,
                Execution::Unknown,
            )));
        }
        let mut response = super::timed_reader::Reader::new(
            response,
            sample.as_ref().and_then(health::Sample::backpressure),
        );
        // Persist delivery intent ONCE before calling any consumer. This is
        // conservative even when the upstream fails before its first text byte.
        delivery.first_output().await?;
        let mut stream =
            crate::endpoint::stream::Stream::new(request, public_id, delivery.created())?;
        let mut saved_upstream_id = false;
        let mut last_read = tokio::time::Instant::now();
        while let Some((chunk, at)) = response.next().await? {
            // One timestamp per network read. Decoding a buffered batch into
            // many SSE events must not manufacture a generation interval.
            last_read = at;
            let blocked = sample.as_ref().and_then(health::Sample::backpressure);
            let mut receive = |frame| {
                let piece = stream.push(frame).map_err(stream_frame_error);
                let future = match piece {
                    Ok(Some(value)) => {
                        if let Some(sample) = sample {
                            sample.delta_at(&value, at);
                        }
                        Ok(Some(health::native::delivery(emit(value), blocked.clone())))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                };
                async move {
                    if let Some(f) = future? {
                        f.await.map_err(|_| worker::Error::Cancelled)?;
                    }
                    Ok(())
                }
            };
            let pushed = decoder.push(&chunk, &mut receive).await;
            drop(receive);
            if !saved_upstream_id {
                if let Some(id) = stream.upstream_id() {
                    delivery.upstream_id(id).await?;
                    saved_upstream_id = true;
                }
            }
            pushed.map_err(|e| {
                if matches!(e, worker::Error::Cancelled) {
                    Error::Cancelled
                } else {
                    Error::Decoder(e)
                }
            })?;
            if stream.is_done() {
                break;
            }
        }
        // [DONE] is terminal in this profile. A conforming long-lived SSE HTTP
        // connection need not close before result verification can finish.
        drop(response);
        if let Some(sample) = sample {
            sample.network_complete(last_read);
        }
        let at = last_read;
        let blocked = sample.as_ref().and_then(health::Sample::backpressure);
        let mut receive = |frame| {
            let piece = stream.push(frame).map_err(stream_frame_error);
            let future = match piece {
                Ok(Some(value)) => {
                    if let Some(sample) = sample {
                        sample.delta_at(&value, at);
                    }
                    Ok(Some(health::native::delivery(emit(value), blocked.clone())))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            async move {
                if let Some(f) = future? {
                    f.await.map_err(|_| worker::Error::Cancelled)?;
                }
                Ok(())
            }
        };
        decoder
            .finish_stream_frames(&mut receive)
            .await
            .map_err(|e| {
                if matches!(e, worker::Error::Cancelled) {
                    Error::Cancelled
                } else {
                    Error::Decoder(e)
                }
            })?;
        drop(receive);
        let result = stream.finish()?;
        decoder.verify_stream_result(&result).await?;
        let reply = request
            .decode_json(result, public_id, delivery.created())
            .map_err(Error::Endpoint)?;
        if let Some(sample) = sample {
            sample.finish_native().await
        }
        Ok(reply)
    }
    pub(super) async fn perform(
        &self,
        request: &Request,
        mut decoder: worker::host::Active,
        public_id: &str,
        created: u64,
        sample: &mut Option<health::Sample>,
    ) -> Result<ProtocolReply> {
        let mut response = self
            .connection
            .send(self.adapter.operation(), Some(request.body().to_vec()))
            .await
            .map_err(Error::Upstream)?;
        if let Some(sample) = sample {
            sample.headers();
        }
        if response.status != 200 || response.format != WireFormat::Json {
            let mut failure = Failure::new(
                Code::UpstreamProtocol,
                Scope::Model,
                Stage::ResponseHeaders,
                Execution::Unknown,
            );
            failure.upstream_status = Some(response.status);
            return Err(Error::Upstream(failure));
        }
        let mut last_read = tokio::time::Instant::now();
        while let Some((chunk, at)) = response.next_timed_chunk().await.map_err(Error::Upstream)? {
            last_read = at;
            decoder
                .push(&chunk, |_| async { Err(worker::Error::Protocol) })
                .await?;
        }
        if let Some(sample) = sample {
            sample.network_complete(last_read);
        }
        let mut value = None;
        decoder
            .finish(|decoded| {
                let accepted = match decoded {
                    Decoded::Json { value: v } if value.is_none() => {
                        value = Some(v);
                        true
                    }
                    _ => false,
                };
                async move {
                    if accepted {
                        Ok(())
                    } else {
                        Err(worker::Error::Protocol)
                    }
                }
            })
            .await?;
        let value = value.ok_or(Error::Decoder(worker::Error::Protocol))?;
        request
            .decode_json(value, public_id, created)
            .map_err(Error::Endpoint)
    }
}
