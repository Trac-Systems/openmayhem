use super::*;
use crate::{
    attempts::FailureSnapshot,
    connector::failure::{Code, Execution, Failure, Scope, Stage},
    exchange::{Message, PublicState, Session},
};
use tokio::{sync::mpsc, task::JoinSet, time::Instant};

pub(super) async fn run(
    owner: Arc<Inner>,
    mut channel: negotiation::Channel,
    mut guard: ConnectionGuard,
    mut stop: watch::Receiver<bool>,
) -> Result<End> {
    let context = channel.context().clone();
    let negotiated = tokio::select! {
        result=negotiate(&owner,&mut channel)=>result,
        _=stopped(&mut stop)=>{
            let _=owner.proposals.cancel_unsigned(context).await;
            return Ok(End::Disconnected)
        },
    };
    let negotiated = match negotiated {
        Ok(value) => value,
        Err(error) => {
            let _ = owner.proposals.cancel_unsigned(context).await;
            return Err(error);
        }
    };
    if !negotiated {
        let _ = owner.proposals.cancel_unsigned(context).await;
        return Ok(End::Refused);
    }
    let paid = channel.into_paid(&owner.identity)?;
    let session = paid.session().clone();
    let (mut sender, mut receiver) = paid.into_duplex()?;
    let (output, mut outgoing) = queue::channel(owner.limits);
    let (incoming_tx, mut incoming) = mpsc::channel(1);
    let mut io = JoinSet::new();
    io.spawn(async move {
        loop {
            let value = receiver.receive(None).await;
            let failed = value.is_err();
            if incoming_tx.send(value).await.is_err() || failed {
                break;
            }
        }
    });
    io.spawn(async move {
        while let Some(entry) = outgoing.recv().await {
            if sender.send(&entry.message).await.is_err() {
                break;
            }
            if let Some(delivered) = entry.delivered {
                let _ = delivered.send(());
            }
        }
    });
    let mut job: Option<JoinHandle<Result<()>>> = None;
    let mut idle = Instant::now() + owner.limits.control_wait;
    let result = loop {
        tokio::select! {
            _=stopped(&mut stop)=>break Ok(End::Disconnected),
            _=tokio::time::sleep_until(idle),if job.is_none()=>{
                match owner.cancellation(session.invocation()) {
                    Ok(None)=>break Ok(End::ControlIdle),
                    Ok(Some(_))=>(),
                    Err(error)=>break Err(error),
                }
                idle=Instant::now()+owner.limits.control_wait;
            },
            _=io.join_next()=>break Ok(End::Disconnected),
            result=async {match &mut job {Some(j)=>j.await,None=>std::future::pending().await}}=>{
                job=None;
                idle=Instant::now()+owner.limits.control_wait;
                if result.is_err() {break Err(Error::Task)}
                // A delivery error leaves its retained result/receipt recoverable.
                if matches!(result,Ok(Err(_))) {break Ok(End::Disconnected)}
            },
            command=incoming.recv()=>{
                let Some(Ok(command))=command else {break Ok(End::Disconnected)};
                idle=Instant::now()+owner.limits.control_wait;
                let result=handle(&owner,&session,command,&output,&mut job).await;
                match result {Ok(true)=>break Ok(End::Settled),Ok(false)=>(),Err(error)=>break Err(error)}
            }
        }
    };
    // Release connection identity before retaining a background execution owner:
    // reconnect may query/cancel its same durable request, never dispatch a second.
    guard.unregister();
    io.abort_all();
    while io.join_next().await.is_some() {}
    drop(output);
    if let Some(job) = job {
        let _ = job.await;
    }
    // No effect for signed/uncertain terms; only never-signed proposal cleanup.
    let _ = owner.proposals.cancel_unsigned(context).await;
    result
}

async fn negotiate(owner: &Inner, channel: &mut negotiation::Channel) -> Result<bool> {
    let context = channel.context().clone();
    let first = channel.receive(owner.limits.control_wait).await?;
    let accepted = match first.message() {
        negotiation::Message::Request { .. } => {
            match owner.proposals.propose(context.clone(), first).await {
                Ok(proposal) => {
                    channel
                        .send(&negotiation::Message::Proposal { proposal })
                        .await?
                }
                Err(error) => {
                    refuse(channel, Some(error)).await?;
                    return Ok(false);
                }
            }
            let offer = channel.receive(owner.limits.control_wait).await?;
            owner
                .proposals
                .accept(context.clone(), offer, crate::supervisor::unix_ms())
                .await
                .map(Some)
        }
        negotiation::Message::Recover => owner.proposals.recover(&context).await,
        _ => return Err(Error::Configuration),
    };
    let value = match accepted {
        Ok(Some(value)) => value,
        result => {
            refuse(channel, result.err()).await?;
            return Ok(false);
        }
    };
    channel
        .send(&negotiation::Message::Accepted { value })
        .await?;
    Ok(true)
}
async fn refuse(channel: &mut negotiation::Channel, error: Option<crate::Error>) -> Result<()> {
    let failure = match error {
        Some(crate::Error::ProviderRequest(crate::endpoint::Error::Request(failure))) => failure,
        Some(crate::Error::ProviderCapacity(
            crate::capacity::Error::Busy | crate::capacity::Error::Quota,
        )) => Failure::new(
            Code::LocalCapacity,
            Scope::Request,
            Stage::BeforeDispatch,
            Execution::Unknown,
        ),
        None => Failure::new(
            Code::RecoveryRequired,
            Scope::Request,
            Stage::BeforeDispatch,
            Execution::Unknown,
        ),
        _ => Failure::new(
            Code::ProviderUnavailable,
            Scope::Request,
            Stage::BeforeDispatch,
            Execution::Unknown,
        ),
    };
    channel
        .send(&negotiation::Message::Refused {
            failure: (&failure).into(),
        })
        .await?;
    Ok(())
}
pub(super) async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() || stop.changed().await.is_err() {
            return;
        }
    }
}

async fn handle(
    owner: &Arc<Inner>,
    session: &Session,
    command: exchange::Received,
    output: &queue::Output,
    job: &mut Option<JoinHandle<Result<()>>>,
) -> Result<bool> {
    match command.message() {
        Message::Execute { request, streaming } => {
            if mayhem_proto::endpoint_request_fingerprint(request)
                != session.authorization().terms.request_hash
            {
                return Err(Error::Transport(exchange::Error::Identity));
            }
            let streaming = *streaming;
            let work = owner.job(session.invocation());
            let (guard, cancel) = match work {
                Ok(value) => value,
                Err(Error::Busy) => {
                    output.control(Message::State {
                        state: PublicState::Running,
                    })?;
                    return Ok(false);
                }
                Err(error) => return Err(error),
            };
            let owner = owner.clone();
            let session = session.clone();
            let output = output.clone();
            *job = Some(tokio::spawn(async move {
                let _guard = guard;
                let result = if streaming {
                    session.execute_stream(command,&owner.executor,&cancel,|event| {
                        let output=output.clone();async move {output.send(Message::Stream{event}).await.map_err(|_|())}
                    }).await
                } else {
                    session
                        .execute_json(command, &owner.executor, &cancel)
                        .await
                };
                match result {
                    Ok(value) => {
                        // Capacity completion depends on durable execution evidence, not delivery/ACK.
                        owner
                            .executor
                            .reconcile_capacity(session.invocation(), value.attempt.attempt)
                            .await?;
                        output
                            .send(Message::Result {
                                response: value.reply.body,
                            })
                            .await?;
                        receipt(&owner, &session, &output, value.attempt.attempt).await?;
                    }
                    Err(exchange::Error::Execution(execution::Error::ExistingResult)) => {
                        recover(&owner, &session, &output).await?;
                    }
                    Err(error) => {
                        let failure = classify(error);
                        output.send(Message::Failure { failure }).await?;
                        offer_non_execution(&owner, &session, &output).await?;
                    }
                }
                Ok(())
            }));
        }
        Message::Status => {
            let (mut state, result) = session.status(command, &owner.executor).await?;
            if state == PublicState::OutcomeUnknown
                && owner.cancellation(session.invocation())?.is_some()
            {
                state = PublicState::Running;
            }
            output.control(Message::State { state })?;
            if let Some(response) = result {
                output.control(Message::Result { response })?;
                let saved = session
                    .existing(&owner.executor)
                    .await?
                    .ok_or(Error::Configuration)?;
                receipt(owner, session, output, saved.record.attempt).await?;
            } else if state != PublicState::Settled {
                offer_non_execution(owner, session, output).await?;
            }
        }
        Message::Cancel => {
            let cancel = owner
                .cancellation(session.invocation())?
                .unwrap_or_default();
            session.cancel(command, &owner.executor, &cancel).await?;
            output.control(Message::State {
                state: PublicState::CancelRequested,
            })?;
            offer_non_execution(owner, session, output).await?;
        }
        Message::Acknowledge { .. } => {
            let saved = session
                .existing(&owner.executor)
                .await?
                .ok_or(Error::Configuration)?;
            let confirmed = session
                .acknowledge(command, &owner.executor, saved.record.attempt)
                .await?;
            output
                .deliver(Message::State {
                    state: if confirmed {
                        PublicState::Settled
                    } else {
                        PublicState::AwaitingReceipt
                    },
                })
                .await?;
            return Ok(confirmed);
        }
        _ => return Err(Error::Transport(exchange::Error::Protocol)),
    }
    Ok(false)
}
async fn recover(owner: &Inner, session: &Session, output: &queue::Output) -> Result<()> {
    let saved = session
        .existing(&owner.executor)
        .await?
        .ok_or(Error::Configuration)?;
    if let Some(result) = saved.result {
        output
            .send(Message::Result {
                response: result.reply.body,
            })
            .await?;
    }
    receipt(owner, session, output, saved.record.attempt).await
}
async fn receipt(
    owner: &Inner,
    session: &Session,
    output: &queue::Output,
    attempt: u64,
) -> Result<()> {
    // Outcome selection needs only bounded metadata, not another copy of the
    // prompt and completed response. Signing still verifies retained ownership.
    let saved = owner
        .executor
        .recovery_header(session.invocation(), attempt)
        .await?;
    if saved.record.binding.accepted_terms.as_str()
        != session
            .authorization()
            .terms
            .digest()
            .map_err(|_| Error::Configuration)?
    {
        return Err(Error::Configuration);
    }
    let message = if saved.record.non_execution_evidence().is_some() {
        owner
            .executor
            .reconcile_capacity(session.invocation(), attempt)
            .await?;
        owner
            .executor
            .sign_waiver(&owner.signer, session.invocation(), attempt)
            .await
            .map(|value| Message::Waiver { value })
    } else {
        owner
            .executor
            .sign_terminal_receipt(&owner.signer, session.invocation(), attempt)
            .await
            .map(|value| Message::Receipt { value })
    };
    match message {
        Ok(value) => output.send(value).await?,
        Err(_) => {
            output
                .send(Message::State {
                    state: PublicState::AwaitingReceipt,
                })
                .await?
        }
    }
    Ok(())
}
/// A failure is not itself a waiver. Only the exact retained proof may offer a
/// zero-charge closure for the buyer to independently approve and countersign.
async fn offer_non_execution(
    owner: &Inner,
    session: &Session,
    output: &queue::Output,
) -> Result<()> {
    if let Some(saved) = session.existing(&owner.executor).await? {
        if saved.record.phase != crate::attempts::Phase::Closed
            && saved.record.non_execution_evidence().is_some()
        {
            receipt(owner, session, output, saved.record.attempt).await?;
        }
    }
    Ok(())
}
fn classify(error: exchange::Error) -> FailureSnapshot {
    let failure = match error {
        exchange::Error::Execution(execution::Error::Upstream(value)) => value,
        exchange::Error::Execution(execution::Error::Endpoint(
            crate::endpoint::Error::Request(value),
        )) => value,
        exchange::Error::Execution(execution::Error::Financial(_)) => Failure::new(
            Code::AdmissionUnavailable,
            Scope::Request,
            Stage::BeforeDispatch,
            Execution::Unknown,
        ),
        exchange::Error::Execution(
            execution::Error::RecoveryRequired | execution::Error::ExistingResult,
        ) => Failure::new(
            Code::RecoveryRequired,
            Scope::Request,
            Stage::Dispatch,
            Execution::Unknown,
        ),
        exchange::Error::Execution(execution::Error::Cancelled) => Failure::new(
            Code::RequestCancelled,
            Scope::Request,
            Stage::Dispatch,
            Execution::Unknown,
        ),
        exchange::Error::Execution(
            execution::Error::Capacity(_) | execution::Error::StorageCapacity,
        ) => Failure::new(
            Code::LocalCapacity,
            Scope::Request,
            Stage::Dispatch,
            Execution::Unknown,
        ),
        exchange::Error::Execution(
            execution::Error::Endpoint(_) | execution::Error::Decoder(_),
        ) => Failure::new(
            Code::UpstreamProtocol,
            Scope::Request,
            Stage::ResponseBody,
            Execution::Unknown,
        ),
        _ => Failure::new(
            Code::ProviderUnavailable,
            Scope::Request,
            Stage::Dispatch,
            Execution::Unknown,
        ),
    };
    (&failure).into()
}
