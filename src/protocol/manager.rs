// ABOUTME: Managed connection lifecycle: concurrent inbound handshakes, spec
// ABOUTME: multi-server arbitration, and automatic goodbye(another_server).

use crate::error::Error;
use crate::protocol::client::{
    ArtworkChunk, AudioChunk, Connection, ConnectionGuard, Controller, SessionInfo, Source,
    VisualizerChunk, WsSender,
};
use crate::protocol::listener::ProtocolListener;
use crate::protocol::messages::{
    Activity, ClientGoodbye, GoodbyeReason, Message, PairAbort, PairAbortReason, PlayerState,
};
use crate::protocol::session::activity_rank;
use crate::sync::ClockSync;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinHandle;

/// The live facts about the current (incumbent) connection that multi-server
/// arbitration compares against. Obtained from
/// [`ConnectionGuard::arbitration_state`]; the activities track the *latest*
/// `server/activate` (spec: servers drop activities as purposes end, and
/// admission is decided on the declared set).
#[derive(Debug, Clone)]
pub struct ArbitrationState {
    /// The incumbent server's identity.
    pub server_id: String,
    /// Activities from the incumbent's latest admissible `server/activate`.
    pub activities: Vec<Activity>,
    /// Whether a pairing attempt is in progress on the incumbent connection.
    /// Spec: a pairing attempt is not displaced by an incoming `'playback'`
    /// or `'pairing'` connection.
    pub pairing_attempt_in_progress: bool,
}

/// The spec's multi-server arbitration rule: should the newly established
/// `candidate` displace the `current` server?
///
/// [`ConnectionManager`] applies this automatically; it is public so
/// applications running their own [`ProtocolListener::accept`] loop can make
/// the identical decision (build `current` via
/// [`ConnectionGuard::arbitration_state`]).
pub fn should_switch(
    current: &ArbitrationState,
    candidate: &SessionInfo,
    last_played: Option<&str>,
) -> bool {
    // Connections are ranked by their highest-ranked declared activity:
    // management > playback > pairing > empty. Higher or equal is accepted.
    let candidate_rank = activity_rank(&candidate.initial_activities);
    let current_rank = activity_rank(&current.activities);
    // Exception: an in-progress pairing attempt is not displaced by an
    // incoming 'playback' or 'pairing' connection (management still wins).
    if current.pairing_attempt_in_progress && candidate_rank <= activity_rank(&[Activity::Playback])
    {
        return false;
    }
    if candidate_rank != current_rank {
        return candidate_rank > current_rank;
    }
    // Both-empty exception: the incoming connection is admitted only when its
    // server_id matches the last-playback server and the existing one's does
    // not; otherwise the existing connection is kept.
    if candidate_rank == 0 {
        return matches!(
            last_played,
            Some(lp) if candidate.server_id == lp && current.server_id != lp
        );
    }
    true
}

/// The spec farewell for an arbitration loser: `pair/abort
/// (concurrent_attempt)` when the losing connection is a pairing handshake,
/// otherwise `client/goodbye` with `reason`.
fn loser_farewell(activities: &[Activity], reason: GoodbyeReason) -> Message {
    if activities.contains(&Activity::Pairing) {
        Message::PairAbort(PairAbort {
            reason: PairAbortReason::ConcurrentAttempt,
        })
    } else {
        Message::ClientGoodbye(ClientGoodbye { reason })
    }
}

/// Default [`ManagerConfig::establish_timeout`]
pub const DEFAULT_ESTABLISH_TIMEOUT: Duration = Duration::from_secs(30);

/// Default [`ManagerConfig::max_concurrent_handshakes`]
pub const DEFAULT_MAX_CONCURRENT_HANDSHAKES: usize = 2;

/// Default [`ManagerConfig::goodbye_timeout`].
pub const DEFAULT_GOODBYE_TIMEOUT: Duration = Duration::from_secs(5);

/// Tuning for [`ConnectionManager`].
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// Deadline for an inbound connection to complete TLS + WebSocket +
    /// protocol handshake, measured from TCP accept. Peers that stall are
    /// dropped. Default: [`DEFAULT_ESTABLISH_TIMEOUT`].
    pub establish_timeout: Duration,
    /// Maximum number of inbound connections concurrently working through
    /// their handshake. Surplus peers wait in the TCP accept backlog rather
    /// than being rejected. Values below 1 are clamped to 1. Default:
    /// [`DEFAULT_MAX_CONCURRENT_HANDSHAKES`].
    pub max_concurrent_handshakes: usize,
    /// Deadline for flushing `client/goodbye` to a losing, displaced, or
    /// disconnected server. On elapse the connection is torn down without
    /// confirmation — a peer that stops reading would otherwise pin the
    /// flush (and its connection's tasks and socket) until the OS TCP
    /// timeout. Default: [`DEFAULT_GOODBYE_TIMEOUT`].
    pub goodbye_timeout: Duration,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            establish_timeout: DEFAULT_ESTABLISH_TIMEOUT,
            max_concurrent_handshakes: DEFAULT_MAX_CONCURRENT_HANDSHAKES,
            goodbye_timeout: DEFAULT_GOODBYE_TIMEOUT,
        }
    }
}

/// Identical in shape to [`Connection`] except the [`ConnectionGuard`] stays
/// with the manager: the manager must retain teardown authority so it can
/// send `client/goodbye (another_server)` to a displaced incumbent.
pub struct ManagedConnection {
    /// JSON protocol messages from the server (bounded; drain promptly).
    pub messages: Receiver<Message>,
    /// Audio chunks from the server (bounded with drop-on-full).
    pub audio: Receiver<AudioChunk>,
    /// Artwork chunks from the server (bounded with drop-on-full).
    pub artwork: Receiver<ArtworkChunk>,
    /// Visualizer chunks from the server (bounded with drop-on-full).
    pub visualizer: Receiver<VisualizerChunk>,
    /// Clock synchronization state. Fresh per connection: audio components
    /// built around it (e.g. `SyncedPlayer`) must be rebuilt per connection.
    pub clock_sync: Arc<Mutex<ClockSync>>,
    /// Sender for writing messages to the server.
    pub sender: WsSender,
    /// Controller handle, if the server granted the `controller@v1` role.
    pub controller: Option<Controller>,
    /// Source handle, if the server granted the `source@v1` role.
    pub source: Option<Source>,
    /// Session facts established during the handshake.
    pub session: SessionInfo,
    /// Peer address of the winning connection.
    pub peer: SocketAddr,
}

impl ManagedConnection {
    /// See [`WsSender::enter_external_source`].
    pub async fn enter_external_source(&self) -> Result<(), Error> {
        self.sender.enter_external_source().await
    }

    /// See [`WsSender::exit_external_source`].
    pub async fn exit_external_source(&self, player: Option<PlayerState>) -> Result<(), Error> {
        self.sender.exit_external_source(player).await
    }
}

/// Commands from the [`ConnectionManager`] handle to its driver task.
enum Command {
    SetLastPlayed(Option<String>),
    Disconnect(GoodbyeReason, oneshot::Sender<Result<(), Error>>),
}

/// The guard stays here so the driver keeps teardown authority over a server
/// the application is still consuming.
struct Incumbent {
    guard: ConnectionGuard,
    session: SessionInfo,
    peer: SocketAddr,
}

/// Owns a [`ProtocolListener`] and drives the full multi-server connection
/// lifecycle:
///
/// - Inbound peers handshake **concurrently** (bounded by
///   [`ManagerConfig::max_concurrent_handshakes`]) with an establish
///   deadline, so a stalling peer can neither block other servers nor occupy
///   a slot forever. A connection is never surfaced before its handshake
///   completes.
/// - Each established connection is arbitrated against the incumbent with
///   [`should_switch`]. The loser — displaced incumbent or rejected
///   newcomer — is automatically sent `client/goodbye (another_server)`.
/// - Winners are yielded from [`Self::next_connection`]. When a winner is
///   displaced (or its server goes away), its channels close and the next
///   call yields the replacement.
///
/// Last-played preference is a policy **input**: persist the server ID in
/// your application and feed it back via [`Self::set_last_played`] (the SDK
/// does not persist state as there is no cross-platform cross-app way to do
/// so cleanly).
///
/// Dropping the manager aborts the accept loop and the current connection
/// **without** a goodbye; call [`Self::disconnect`] first for a graceful
/// shutdown.
///
/// ```no_run
/// # use sendspin::{ConnectionManager, ProtocolClientBuilder};
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let listener = ProtocolClientBuilder::builder()
///     .name("Kitchen Speaker".to_string())
///     .build()
///     .listen("0.0.0.0:8927")
///     .await?;
/// let mut manager = ConnectionManager::new(listener);
/// while let Some(conn) = manager.next_connection().await {
///     // Serve conn until its channels close, then loop for its successor.
/// }
/// # Ok(()) }
/// ```
///
/// See `examples/server_initiated_metadata.rs` for a complete program.
pub struct ConnectionManager {
    conn_rx: mpsc::UnboundedReceiver<ManagedConnection>,
    cmd_tx: mpsc::UnboundedSender<Command>,
    accept_task: JoinHandle<()>,
    driver_task: JoinHandle<()>,
    local_addr: Option<SocketAddr>,
}

impl std::fmt::Debug for ConnectionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionManager")
            .field("local_addr", &self.local_addr)
            .finish()
    }
}

impl ConnectionManager {
    /// Manage `listener` with default [`ManagerConfig`].
    pub fn new(listener: ProtocolListener) -> Self {
        Self::with_config(listener, ManagerConfig::default())
    }

    /// Manage `listener` with explicit tuning.
    pub fn with_config(listener: ProtocolListener, mut config: ManagerConfig) -> Self {
        // Zero handshake slots would deadlock the accept loop; clamp once
        // here so every downstream use sees the same normalized value.
        config.max_concurrent_handshakes = config.max_concurrent_handshakes.max(1);

        let local_addr = listener.local_addr().ok();
        // Bounded: pairs with the handshake slots to stop admission when the
        // arbitration driver stalls (see accept_loop).
        let (established_tx, established_rx) =
            mpsc::channel::<(Connection, SocketAddr)>(config.max_concurrent_handshakes);
        let (conn_tx, conn_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        let goodbye_timeout = config.goodbye_timeout;
        let accept_task = tokio::spawn(accept_loop(Arc::new(listener), established_tx, config));
        let driver_task = tokio::spawn(driver(established_rx, conn_tx, cmd_rx, goodbye_timeout));

        Self {
            conn_rx,
            cmd_tx,
            accept_task,
            driver_task,
            local_addr,
        }
    }

    /// Wait for the next connection to win arbitration. Cancel-safe.
    /// Returns `None` only if the internal driver has stopped (it does not
    /// stop in normal operation).
    ///
    /// Winners queue unboundedly, so consume promptly. An entry displaced
    /// before you received it arrives with already-closed channels — drain
    /// it and loop again.
    pub async fn next_connection(&mut self) -> Option<ManagedConnection> {
        self.conn_rx.recv().await
    }

    /// Set (or clear, with `None`) the last-played server ID used to break
    /// ties between two `discovery` connections. Persisting this across runs
    /// is the application's responsibility.
    pub fn set_last_played(&self, server_id: Option<String>) {
        // Send failure means the driver is gone; arbitration is moot then.
        let _ = self.cmd_tx.send(Command::SetLastPlayed(server_id));
    }

    /// Gracefully disconnect the current server, sending `client/goodbye`
    /// with `reason` and awaiting the flush (bounded by
    /// [`ManagerConfig::goodbye_timeout`]). No-op `Ok(())` when no server
    /// is connected. The manager keeps listening; a later inbound server
    /// is yielded from [`Self::next_connection`] as usual.
    pub async fn disconnect(&self, reason: GoodbyeReason) -> Result<(), Error> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::Disconnect(reason, ack_tx))
            .map_err(|_| Error::Connection("connection manager stopped".to_string()))?;
        ack_rx
            .await
            .map_err(|_| Error::Connection("connection manager stopped".to_string()))?
    }

    /// Local bound address of the underlying listener, if it was available
    /// at construction time.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }
}

impl Drop for ConnectionManager {
    fn drop(&mut self) {
        // Abrupt teardown: aborting the driver drops the incumbent's guard,
        // which aborts its background tasks without a goodbye. Graceful
        // shutdown is `disconnect(...)` before drop.
        self.accept_task.abort();
        self.driver_task.abort();
    }
}

/// Admit TCP peers as slots allow and run each handshake on its own task
/// under the establish deadline. Surplus peers wait in the TCP accept backlog
/// rather than being rejected with a goodbye.
///
/// A task holds its slot through the `established_tx` send, so a stalled
/// arbitration driver stops admission instead of accumulating established
/// connections. Tasks live in a `JoinSet` so aborting this loop (manager
/// drop) aborts in-flight handshakes, releasing their sockets — and the
/// listener's port — promptly instead of after `establish_timeout`.
async fn accept_loop(
    listener: Arc<ProtocolListener>,
    established_tx: mpsc::Sender<(Connection, SocketAddr)>,
    config: ManagerConfig,
) {
    let slots = Arc::new(Semaphore::new(config.max_concurrent_handshakes));
    let mut handshakes = tokio::task::JoinSet::new();
    loop {
        while handshakes.try_join_next().is_some() {}

        let permit = Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("handshake semaphore is never closed");

        let (tcp, peer) = match listener.accept_tcp().await {
            Ok(accepted) => accepted,
            Err(e) => {
                // Accept errors (e.g. fd exhaustion) are usually transient;
                // pause briefly instead of spinning or dying.
                log::warn!("ConnectionManager accept failed: {e}");
                drop(permit);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };

        let listener = Arc::clone(&listener);
        let established_tx = established_tx.clone();
        let deadline = config.establish_timeout;
        handshakes.spawn(async move {
            // Hold the handshake slot for the lifetime of the attempt.
            let _permit = permit;
            match tokio::time::timeout(deadline, listener.handshake_and_drive(tcp)).await {
                Ok(Ok(client)) => {
                    // Driver gone (manager dropped): the connection is
                    // dropped here and its guard aborts its tasks.
                    let _ = established_tx.send((client.split(), peer)).await;
                }
                Ok(Err(e)) => log::warn!("Inbound handshake from {peer} failed: {e}"),
                Err(_) => log::warn!(
                    "Inbound connection from {peer} did not establish within {deadline:?}; dropping"
                ),
            }
        });
    }
}

/// Arbitration driver: single owner of the incumbent connection's guard and
/// the last-played policy input. All lifecycle transitions happen here.
async fn driver(
    mut established_rx: mpsc::Receiver<(Connection, SocketAddr)>,
    conn_tx: mpsc::UnboundedSender<ManagedConnection>,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    goodbye_timeout: Duration,
) {
    let mut last_played: Option<String> = None;
    let mut current: Option<Incumbent> = None;
    // Goodbye flushes run in a JoinSet rather than detached, so aborting the
    // driver (manager drop) also aborts any still-wedged flush.
    let mut goodbyes = tokio::task::JoinSet::new();

    loop {
        while goodbyes.try_join_next().is_some() {}

        // Resolves when the incumbent's reader task ends (server closed the
        // socket or transport failure); pends forever when there is none.
        let incumbent_closed = async {
            match current.as_mut() {
                Some(inc) => inc.guard.closed().await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            established = established_rx.recv() => {
                let Some((conn, peer)) = established else {
                    // Accept loop is gone; only possible when the manager
                    // handle was dropped, which also aborts this task.
                    break;
                };
                arbitrate(
                    &mut current,
                    conn,
                    peer,
                    last_played.as_deref(),
                    &conn_tx,
                    &mut goodbyes,
                    goodbye_timeout,
                );
            }
            _ = incumbent_closed => {
                let inc = current.take().expect("closed() only fires with an incumbent");
                log::info!(
                    "Server {} ({}) disconnected; awaiting next server",
                    inc.session.server_id,
                    inc.peer
                );
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break, // ConnectionManager handle dropped.
                    Some(Command::SetLastPlayed(id)) => last_played = id,
                    Some(Command::Disconnect(reason, ack)) => {
                        match current.take() {
                            // Peer already dead: a goodbye is moot, and
                            // flushing one at a dead socket would turn a
                            // successful teardown into a spurious error.
                            Some(inc) if inc.guard.is_closed() => {
                                let _ = ack.send(Ok(()));
                            }
                            // Off the driver task so a slow flush can't
                            // stall arbitration; the caller still gets the
                            // real result via the ack.
                            Some(inc) => {
                                let farewell =
                                    Message::ClientGoodbye(ClientGoodbye { reason });
                                goodbyes.spawn(async move {
                                    let _ = ack.send(
                                        flush_farewell(inc.guard, farewell, goodbye_timeout).await,
                                    );
                                });
                            }
                            None => {
                                let _ = ack.send(Ok(()));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Flush a farewell (`client/goodbye` or `pair/abort`) with a deadline: a
/// peer that stops reading must not pin the flush — and the connection's
/// tasks and socket — until the OS TCP timeout. On elapse the guard is
/// dropped, aborting the connection.
async fn flush_farewell(
    guard: ConnectionGuard,
    farewell: Message,
    deadline: Duration,
) -> Result<(), Error> {
    guard.farewell(farewell, deadline).await
}

/// Apply [`should_switch`] to a freshly established connection: promote it
/// (goodbying any displaced incumbent) or reject it with a goodbye.
fn arbitrate(
    current: &mut Option<Incumbent>,
    conn: Connection,
    peer: SocketAddr,
    last_played: Option<&str>,
    conn_tx: &mpsc::UnboundedSender<ManagedConnection>,
    goodbyes: &mut tokio::task::JoinSet<()>,
    goodbye_timeout: Duration,
) {
    // The incumbent may have died in the same instant this connection
    // established, and the select loop can deliver the establishment first.
    // A dead incumbent must not reject a live server, so check liveness
    // before applying policy. No goodbye owed: the transport is gone.
    if current.as_ref().is_some_and(|inc| inc.guard.is_closed()) {
        let dead = current.take().expect("checked Some above");
        log::info!(
            "Server {} ({}) already disconnected; arbitration proceeds without it",
            dead.session.server_id,
            dead.peer
        );
    }

    if let Some(inc) = current.as_ref() {
        if !should_switch(&inc.guard.arbitration_state(), &conn.session, last_played) {
            // A rejected incoming connection receives goodbye
            // 'concurrent_attempt' — or pair/abort (concurrent_attempt) when
            // it is a pairing handshake (spec: Multiple servers).
            log::info!(
                "Keeping server {} — rejecting {} ({peer}) as concurrent_attempt",
                inc.session.server_id,
                conn.session.server_id,
            );
            let farewell = loser_farewell(
                &conn.guard.arbitration_state().activities,
                GoodbyeReason::ConcurrentAttempt,
            );
            // Spawned so a slow flush can't stall arbitration; dropping the
            // rest of `conn` closes its channels.
            goodbyes.spawn(async move {
                let _ = flush_farewell(conn.guard, farewell, goodbye_timeout).await;
            });
            return;
        }

        let displaced = current.take().expect("checked Some above");
        log::info!(
            "Switching {} -> {} — farewell(another_server) to displaced server",
            displaced.session.server_id,
            conn.session.server_id,
        );
        let farewell = loser_farewell(
            &displaced.guard.arbitration_state().activities,
            GoodbyeReason::AnotherServer,
        );
        goodbyes.spawn(async move {
            let _ = flush_farewell(displaced.guard, farewell, goodbye_timeout).await;
        });
    } else {
        log::info!(
            "Server {} ({peer}) connected (activities: {:?})",
            conn.session.server_id,
            conn.session.initial_activities,
        );
    }

    let Connection {
        messages,
        audio,
        artwork,
        visualizer,
        clock_sync,
        sender,
        controller,
        source,
        session,
        guard,
    } = conn;

    *current = Some(Incumbent {
        guard,
        session: session.clone(),
        peer,
    });

    // Receiver dropped means the manager handle is gone; the driver will
    // exit via its command channel shortly after.
    let _ = conn_tx.send(ManagedConnection {
        messages,
        audio,
        artwork,
        visualizer,
        clock_sync,
        sender,
        controller,
        source,
        session,
        peer,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::crypto::CipherSuite;
    use crate::protocol::messages::{Activity, TrustLevel};

    fn session(server_id: &str, activities: Vec<Activity>) -> SessionInfo {
        SessionInfo {
            server_id: server_id.to_string(),
            server_name: format!("{server_id} name"),
            trust_level: TrustLevel::None,
            suite: CipherSuite::ChaChaPoly,
            initial_activities: activities,
            initial_active_roles: vec![],
        }
    }

    fn state(server_id: &str, activities: Vec<Activity>) -> ArbitrationState {
        ArbitrationState {
            server_id: server_id.to_string(),
            activities,
            pairing_attempt_in_progress: false,
        }
    }

    #[test]
    fn higher_or_equal_rank_wins() {
        // playback displaces playback (equal rank: incoming accepted)
        let current = state("a", vec![Activity::Playback]);
        let candidate = session("b", vec![Activity::Playback]);
        assert!(should_switch(&current, &candidate, None));

        // management displaces playback
        let candidate = session("b", vec![Activity::Management]);
        assert!(should_switch(&current, &candidate, None));

        // playback displaces empty
        let current = state("a", vec![]);
        let candidate = session("b", vec![Activity::Playback]);
        assert!(should_switch(&current, &candidate, None));
    }

    #[test]
    fn lower_rank_never_displaces() {
        let current = state("a", vec![Activity::Playback]);
        let candidate = session("b", vec![]);
        assert!(!should_switch(&current, &candidate, None));
        // Not even when the candidate is the last-playback server.
        assert!(!should_switch(&current, &candidate, Some("b")));

        let current = state("a", vec![Activity::Management]);
        let candidate = session("b", vec![Activity::Pairing]);
        assert!(!should_switch(&current, &candidate, None));
    }

    #[test]
    fn both_empty_prefers_last_playback_server() {
        let current = state("a", vec![]);
        let candidate = session("b", vec![]);
        assert!(should_switch(&current, &candidate, Some("b")));
        assert!(!should_switch(&current, &candidate, Some("a")));
        assert!(!should_switch(&current, &candidate, None));
        assert!(!should_switch(&current, &candidate, Some("c")));
    }

    #[test]
    fn both_empty_keeps_existing_when_it_matches_last_playback() {
        // "and the existing one's does not": if the existing connection is
        // already the last-playback server, keep it.
        let current = state("a", vec![]);
        let candidate = session("a", vec![]);
        assert!(!should_switch(&current, &candidate, Some("a")));
    }

    #[test]
    fn pairing_attempt_is_not_displaced_by_playback_or_pairing() {
        let mut current = state("a", vec![Activity::Pairing]);
        current.pairing_attempt_in_progress = true;

        // Neither playback nor another pairing displaces an in-progress
        // pairing attempt, despite outranking or equalling it.
        let playback = session("b", vec![Activity::Playback]);
        assert!(!should_switch(&current, &playback, None));
        let pairing = session("b", vec![Activity::Pairing]);
        assert!(!should_switch(&current, &pairing, None));

        // Management still wins.
        let management = session("b", vec![Activity::Management]);
        assert!(should_switch(&current, &management, None));

        // Without an in-progress attempt, normal ranking applies.
        current.pairing_attempt_in_progress = false;
        assert!(should_switch(&current, &playback, None));
    }

    #[test]
    fn loser_farewell_picks_pair_abort_for_pairing_handshakes() {
        assert!(matches!(
            loser_farewell(&[Activity::Pairing], GoodbyeReason::ConcurrentAttempt),
            Message::PairAbort(PairAbort {
                reason: PairAbortReason::ConcurrentAttempt
            })
        ));
        assert!(matches!(
            loser_farewell(&[Activity::Playback], GoodbyeReason::AnotherServer),
            Message::ClientGoodbye(ClientGoodbye {
                reason: GoodbyeReason::AnotherServer
            })
        ));
    }
}
