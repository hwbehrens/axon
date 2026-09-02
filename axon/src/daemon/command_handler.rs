use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ipc::{
    CommandEvent, DaemonReply, IpcCommand, IpcErrorCode, IpcSendKind, IpcServer, PeerSummary,
};
use crate::message::{AgentId, Envelope};
use crate::peer_directory::{DirectoryError, PeerDirectory, PeerIdentity, PeerLocator, PeerTrust};
use crate::peer_token;
use crate::request_broker::{BrokerError, RequestBroker};
use crate::transport::{ConnectionManager, REQUEST_TIMEOUT};

use super::who_can;

/// Upper bound for caller-supplied `timeout_secs`. Without it, `u64::MAX`
/// would overflow `Instant::now() + timeout` into a panic that leaks the
/// sender's reserved send-slot budget. One hour comfortably exceeds any
/// legitimate interactive request while keeping the arithmetic safe.
const MAX_REQUEST_TIMEOUT_SECS: u64 = 3600;

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) sent: AtomicU64,
    pub(crate) received: AtomicU64,
}

#[derive(Debug)]
struct CommandFailure {
    code: IpcErrorCode,
    message: String,
}

impl CommandFailure {
    fn new(code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Map typed directory failures onto instructive IPC error codes
/// (spec/IPC.md §5): unknown-peer classes stay user-facing
/// (`peer_not_found`, `peer_not_observed`), while capacity and persistence
/// failures are `internal_error`. The round-six behavior mapped EVERY
/// directory error onto the not-found classes, reporting a failed disk save
/// or an exhausted enrolled-peer bound as a missing peer.
fn directory_failure(err: DirectoryError) -> CommandFailure {
    let code = match &err {
        DirectoryError::NotEnrolled(_) => IpcErrorCode::PeerNotFound,
        DirectoryError::NotObserved(_) => IpcErrorCode::PeerNotObserved,
        DirectoryError::LocalAgentId(_) => IpcErrorCode::SelfSend,
        DirectoryError::EnrolledCapacity
        | DirectoryError::LocatorCapacity
        | DirectoryError::Persist(_) => IpcErrorCode::InternalError,
    };
    CommandFailure::new(code, err.to_string())
}

#[derive(Clone)]
pub(crate) struct DaemonContext {
    pub(crate) ipc: IpcServer,
    pub(crate) directory: PeerDirectory,
    pub(crate) transport: ConnectionManager,
    pub(crate) broker: RequestBroker,
    pub(crate) local_agent_id: AgentId,
    pub(crate) counters: std::sync::Arc<Counters>,
    /// In-flight `send` tasks; control commands never consume this budget.
    pub(crate) inflight_sends: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) max_inflight_sends: usize,
    pub(crate) start: Instant,
    /// Runtime cache of connected peers' capability manifests (advisory).
    pub(crate) manifest_cache: crate::manifest::ManifestCache,
}

pub(crate) async fn handle_command(cmd: CommandEvent, ctx: &DaemonContext) -> Result<()> {
    let client_id = cmd.client_id;
    let reply = match cmd.command {
        IpcCommand::Send {
            to,
            kind,
            payload,
            timeout_secs,
            ref_id,
            req_id,
        } => {
            // Reserve a send slot atomically before executing. The budget
            // counts exactly the sends being processed, so a limit of N
            // admits N concurrent sends and rejects the rest.
            if reserve_send_slot(ctx).is_none() {
                // Control commands stay responsive under send pressure: only
                // excess sends are rejected.
                error_reply(
                    CommandFailure::new(
                        IpcErrorCode::SendCapacityExceeded,
                        IpcErrorCode::SendCapacityExceeded.message(),
                    ),
                    req_id,
                )
            } else {
                // Panic-safe permit: the guard decrements the counter on scope
                // exit AND on unwind, so a panicking handler task cannot leak
                // slots until the budget is permanently exhausted.
                let _slot_guard = SendSlotGuard(ctx.inflight_sends.clone());
                let outcome = send(ctx, to, kind, payload, timeout_secs, ref_id).await;
                match outcome {
                    Ok((msg_id, response)) => DaemonReply::SendOk {
                        ok: true,
                        msg_id,
                        req_id,
                        response,
                    },
                    Err(failure) => error_reply(failure, req_id),
                }
            }
        }
        IpcCommand::Peers { req_id } => {
            let mut peers = Vec::new();
            for peer in ctx.directory.list().await {
                let connected = ctx.transport.has_connection(peer.identity.agent_id()).await;
                let trust = match peer.trust {
                    PeerTrust::Candidate => "candidate",
                    PeerTrust::Enrolled => "enrolled",
                };
                let status = match (peer.trust, connected) {
                    (PeerTrust::Candidate, _) => "discovered",
                    (PeerTrust::Enrolled, true) => "connected",
                    (PeerTrust::Enrolled, false) => "disconnected",
                };
                let mut locators: Vec<_> = peer
                    .configured_locators
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                locators.extend(peer.observed_endpoints.iter().map(ToString::to_string));
                locators.sort();
                locators.dedup();
                let services =
                    ctx.manifest_cache
                        .get(peer.identity.agent_id())
                        .await
                        .map(|manifest| {
                            manifest
                                .services
                                .iter()
                                .map(|service| service.id.clone())
                                .collect::<Vec<_>>()
                        });
                peers.push(PeerSummary {
                    agent_id: peer.identity.agent_id().to_string(),
                    public_key: peer.identity.public_key().to_string(),
                    trust,
                    locators,
                    status,
                    rtt_ms: None,
                    display_name: peer.display_name.map(Into::into),
                    services,
                });
            }
            DaemonReply::Peers {
                ok: true,
                peers,
                req_id,
            }
        }
        IpcCommand::Status { req_id } => DaemonReply::Status {
            ok: true,
            uptime_secs: ctx.start.elapsed().as_secs(),
            peers_connected: ctx.transport.connected_count().await,
            messages_sent: ctx.counters.sent.load(Ordering::Relaxed),
            messages_received: ctx.counters.received.load(Ordering::Relaxed),
            req_id,
        },
        IpcCommand::Whoami { req_id } => DaemonReply::Whoami {
            ok: true,
            info: ctx.ipc.whoami_info(),
            req_id,
        },
        IpcCommand::AddPeer {
            agent_id,
            token,
            req_id,
        } => match add_peer(ctx, agent_id, token).await {
            Ok(agent_id) => DaemonReply::PeerChanged {
                ok: true,
                agent_id: agent_id.to_string(),
                req_id,
            },
            Err(failure) => error_reply(failure, req_id),
        },
        IpcCommand::RemovePeer { agent_id, req_id } => {
            // revoke_peer is the only sanctioned revocation path: it pairs
            // the directory commit with transport teardown so the admission
            // gate's revocation guarantee cannot be defeated by a skipped
            // close_peer.
            match ctx.transport.revoke_peer(&agent_id).await {
                Ok(_) => DaemonReply::PeerChanged {
                    ok: true,
                    agent_id: agent_id.to_string(),
                    req_id,
                },
                Err(err) => error_reply(directory_failure(err), req_id),
            }
        }
        IpcCommand::Serve { manifest, req_id } => {
            // Schema and encoded-size bounds are enforced at deserialization
            // (manifest::Manifest), so a parsed manifest always satisfies
            // every daemon invariant; serde failures surface as
            // `invalid_command` at the protocol layer.
            match ctx.broker.register(client_id, manifest).await {
                Ok(()) => DaemonReply::Serving {
                    ok: true,
                    serving: true,
                    req_id,
                },
                Err(err) => error_reply(broker_failure(err), req_id),
            }
        }
        IpcCommand::WhoCan { query, req_id } => {
            let mut reply = who_can::handle(ctx, query).await;
            if let DaemonReply::WhoCan { req_id: slot, .. } = &mut reply {
                *slot = req_id;
            }
            reply
        }
        IpcCommand::Reply {
            request_id,
            peer,
            kind,
            payload,
            req_id,
        } => match ctx
            .broker
            .reply(client_id, request_id, kind.as_message_kind(), payload, peer)
            .await
        {
            Ok(()) => DaemonReply::Replied {
                ok: true,
                request_id,
                req_id,
            },
            Err(err) => error_reply(broker_failure(err), req_id),
        },
    };

    ctx.ipc.send_reply(client_id, &reply).await
}

async fn add_peer(
    ctx: &DaemonContext,
    agent_id: Option<AgentId>,
    token: Option<String>,
) -> std::result::Result<AgentId, CommandFailure> {
    match (agent_id, token) {
        (Some(agent_id), None) => {
            if agent_id == ctx.local_agent_id {
                return Err(CommandFailure::new(
                    IpcErrorCode::SelfSend,
                    IpcErrorCode::SelfSend.message(),
                ));
            }
            ctx.directory
                .enroll_candidate(&agent_id)
                .await
                .map(|identity| identity.agent_id().clone())
                .map_err(directory_failure)
        }
        (None, Some(token)) => {
            let decoded = peer_token::decode(&token).map_err(|err| {
                CommandFailure::new(IpcErrorCode::InvalidCommand, err.to_string())
            })?;
            let locator = PeerLocator::parse(&decoded.addr).map_err(|err| {
                CommandFailure::new(IpcErrorCode::InvalidCommand, err.to_string())
            })?;
            let identity = PeerIdentity::from_parts(decoded.agent_id, &decoded.pubkey)
                .map_err(|err| CommandFailure::new(IpcErrorCode::PeerConflict, err.to_string()))?;
            if identity.agent_id() == &ctx.local_agent_id {
                return Err(CommandFailure::new(
                    IpcErrorCode::SelfSend,
                    IpcErrorCode::SelfSend.message(),
                ));
            }
            ctx.directory
                .enroll(identity, vec![locator])
                .await
                .map(|identity| identity.agent_id().clone())
                .map_err(directory_failure)
        }
        _ => Err(CommandFailure::new(
            IpcErrorCode::InvalidCommand,
            "add_peer requires exactly one of agent_id or token",
        )),
    }
}

async fn send(
    ctx: &DaemonContext,
    to: AgentId,
    kind: IpcSendKind,
    payload: serde_json::Value,
    timeout_secs: Option<u64>,
    ref_id: Option<uuid::Uuid>,
) -> std::result::Result<(uuid::Uuid, Option<Envelope>), CommandFailure> {
    if to == ctx.local_agent_id {
        return Err(CommandFailure::new(
            IpcErrorCode::SelfSend,
            IpcErrorCode::SelfSend.message(),
        ));
    }
    if ctx.directory.get_enrolled(&to).await.is_none() {
        return Err(CommandFailure::new(
            IpcErrorCode::PeerNotFound,
            IpcErrorCode::PeerNotFound.message(),
        ));
    }
    let timeout = send_timeout(kind, timeout_secs)?;
    let mut envelope = Envelope::new(
        ctx.local_agent_id.clone(),
        to.clone(),
        kind.as_message_kind(),
        payload,
    );
    envelope.ref_id = ref_id;
    envelope
        .validate()
        .map_err(|err| CommandFailure::new(IpcErrorCode::InvalidCommand, err.to_string()))?;
    let msg_id = envelope.id;

    // No outer tokio::time::timeout here: dropping the send future on a
    // deadline would skip `send_to`'s retirement of the exact failed
    // connection and leave a stale slot registered. The budget is enforced
    // inside the transport (dial, write, and response waits are all
    // deadline-bounded), so every failure returns normally through the
    // retirement path.
    match ctx
        .transport
        .send_to(&ctx.directory, &to, envelope, timeout)
        .await
    {
        Ok(response) => {
            ctx.counters.sent.fetch_add(1, Ordering::Relaxed);
            if response.is_some() {
                ctx.counters.received.fetch_add(1, Ordering::Relaxed);
            }
            Ok((msg_id, response))
        }
        Err(err)
            if err.timed_out && matches!(kind, IpcSendKind::Request | IpcSendKind::Describe) =>
        {
            // Spec: timeouts must surface as `timeout`, never
            // `peer_unreachable`.
            Err(CommandFailure::new(
                IpcErrorCode::Timeout,
                IpcErrorCode::Timeout.message(),
            ))
        }
        Err(err) => Err(CommandFailure::new(
            IpcErrorCode::PeerUnreachable,
            err.inner.to_string(),
        )),
    }
}

fn send_timeout(
    kind: IpcSendKind,
    timeout_secs: Option<u64>,
) -> std::result::Result<Duration, CommandFailure> {
    match (kind, timeout_secs) {
        (IpcSendKind::Request | IpcSendKind::Describe, Some(0)) => Err(CommandFailure::new(
            IpcErrorCode::InvalidCommand,
            "timeout_secs must be at least 1",
        )),
        // Bound before any Instant arithmetic: u64::MAX seconds would panic
        // on `Instant::now() + timeout` inside the transport.
        (IpcSendKind::Request | IpcSendKind::Describe, Some(secs))
            if secs > MAX_REQUEST_TIMEOUT_SECS =>
        {
            Err(CommandFailure::new(
                IpcErrorCode::InvalidCommand,
                "timeout_secs exceeds the maximum allowed value",
            ))
        }
        (IpcSendKind::Request | IpcSendKind::Describe, seconds) => Ok(Duration::from_secs(
            seconds.unwrap_or(REQUEST_TIMEOUT.as_secs()),
        )),
        (IpcSendKind::Message, Some(_)) => Err(CommandFailure::new(
            IpcErrorCode::InvalidCommand,
            "timeout_secs is only valid for request and describe kinds",
        )),
        (IpcSendKind::Message, None) => Ok(Duration::from_secs(10)),
    }
}

fn broker_failure(error: BrokerError) -> CommandFailure {
    let code = match error {
        BrokerError::HandlerBusy => IpcErrorCode::HandlerBusy,
        BrokerError::NotHandler => IpcErrorCode::NotHandler,
        BrokerError::RequestNotFound => IpcErrorCode::RequestNotFound,
        BrokerError::AmbiguousRequest => IpcErrorCode::InvalidCommand,
        BrokerError::InvalidPayload => IpcErrorCode::InvalidCommand,
    };
    CommandFailure::new(code, code.message())
}

fn error_reply(failure: CommandFailure, req_id: Option<String>) -> DaemonReply {
    DaemonReply::Error {
        ok: false,
        error: failure.code,
        message: failure.message,
        req_id,
    }
}

/// RAII holder for one reserved send-capacity slot. Dropping the guard —
/// normally or during unwinding — releases the slot, so a panic in the send
/// path can no longer permanently leak capacity.
struct SendSlotGuard(std::sync::Arc<AtomicUsize>);

impl Drop for SendSlotGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Atomically claim one send-capacity slot. Returns `None` when the budget
/// is exhausted. Compare-and-swap keeps the count consistent when many IPC
/// commands race: the limit bounds exactly the sends being processed.
fn reserve_send_slot(ctx: &DaemonContext) -> Option<()> {
    reserve_slot(&ctx.inflight_sends, ctx.max_inflight_sends)
}

pub(crate) fn reserve_slot(counter: &AtomicUsize, max: usize) -> Option<()> {
    loop {
        let current = counter.load(Ordering::Relaxed);
        if current >= max {
            return None;
        }
        if counter
            .compare_exchange_weak(current, current + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return Some(());
        }
    }
}

#[cfg(test)]
#[path = "command_handler_tests.rs"]
mod tests;
