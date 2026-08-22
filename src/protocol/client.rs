// ABOUTME: WebSocket client: connection establishment, task supervision, and frame routing
// ABOUTME: Protocol decisions live in session.rs; this file owns I/O and lifecycle

use crate::error::Error;
use crate::log_sampling::should_log_sample;
use crate::protocol::crypto::{
    b64url_decode, b64url_decode_32, b64url_encode, select_psk, CipherSuite, Identity, Psk,
    PskCandidate, PskCategory,
};
use crate::protocol::management::ManagementState;
use crate::protocol::manager::ArbitrationState;
use crate::protocol::messages::{
    Activity, ClientGoodbye, ClientInit, ClientState, ClientTime, GoodbyeReason, Message,
    NoiseHandshake, NoiseMessage1Payload, PairAbort, PairAbortReason, TrustLevel,
};
use crate::protocol::pairing::PairingStore;
use crate::protocol::roles::SharedSessionState;
use crate::protocol::session::{
    enqueue_json, evaluate_activate, pairing_method_ok, trust_level_for, ActivateVerdict,
    HelloTemplate, SessionFlow, SessionIo, SessionState,
};
use crate::protocol::transport::{frame_type, ClientHandshake, EncryptedChannel};
use crate::protocol::writer::{send_encrypted, writer_task, OutboundPayload, WriteCommand};
use crate::sync::raw_clock::Clock;
use crate::sync::ClockSync;
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{self, unbounded_channel, Receiver, Sender};
use tokio::sync::{oneshot, watch};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

pub use crate::protocol::binary::{
    binary_types, ArtworkChunk, AudioChunk, BinaryFrame, VisualizerChunk,
};
pub use crate::protocol::roles::{Controller, Source, WsSender};

/// Recommended timeout for each expected message during the handshake phases.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Default deadline for a graceful disconnect: a peer that stops reading must
/// not pin the goodbye flush (and this connection's tasks and socket) until
/// the OS TCP timeout.
pub const DEFAULT_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Buffered control messages before the router applies backpressure. Control
/// traffic is low-rate; a consumer that never drains its `messages` receiver
/// eventually stalls the router (and thereby the connection) instead of
/// growing process memory without bound.
const MESSAGE_CHANNEL_CAPACITY: usize = 1024;

/// Buffered audio chunks (~5s of 20ms chunks). Real-time data: when the
/// consumer falls this far behind, newer chunks are dropped with a warning.
const AUDIO_CHANNEL_CAPACITY: usize = 256;

/// Buffered artwork chunks. Artwork arrives in small bursts on track changes.
const ARTWORK_CHANNEL_CAPACITY: usize = 32;

/// Buffered visualizer chunks. Real-time data with drop-on-full policy.
const VISUALIZER_CHANNEL_CAPACITY: usize = 256;

/// Everything the connection driver needs to establish a session.
pub(crate) struct SessionConfig {
    /// The client's long-lived static identity.
    pub identity: Arc<Identity>,
    /// The Noise cipher suite announced in `client/init`.
    pub suite: CipherSuite,
    /// PSK candidates for `psk_id` selection (sentinel, pairing, records).
    pub psk_candidates: Vec<PskCandidate>,
    /// Persistence for pairing records (new records land here).
    pub store: Arc<dyn PairingStore>,
    /// The client's Pairing PSK, when configured.
    pub pairing_psk: Option<Psk>,
    /// Whether this client currently admits unpaired access.
    pub unpaired_access: bool,
    /// `record_mode.psk_id`: the pre-provisioned shared-PSK fallback record.
    pub record_mode: String,
    /// Template for `client/hello` (trust level is filled per connection).
    pub hello: HelloTemplate,
    /// The initial `client/state` sent after `server/activate`.
    pub initial_state: ClientState,
    /// Monotonic clock used for time sync.
    pub clock: Arc<dyn Clock>,
}

/// Immutable facts about an established session, captured at handshake time.
///
/// `initial_activities` and `initial_active_roles` reflect the *initial*
/// `server/activate` only; later activations are forwarded as
/// [`Message::ServerActivate`] on the message channel and tracked live —
/// query [`Connection::active_roles`] / [`Connection::activities`] for the
/// current values.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// The server's identity (static public key, base64url)
    pub server_id: String,
    /// The server's friendly name from `server/hello`
    pub server_name: String,
    /// Trust level asserted in `client/hello`
    pub trust_level: TrustLevel,
    /// The negotiated cipher suite
    pub suite: CipherSuite,
    /// Activities from the initial `server/activate`
    pub initial_activities: Vec<Activity>,
    /// Active roles from the initial `server/activate`
    pub initial_active_roles: Vec<String>,
}

/// Connection components returned by [`ProtocolClient::split()`].
/// Use the fields you need; ignore the rest.
pub struct Connection {
    /// Protocol messages from the server. Bounded: the router applies
    /// backpressure when this queue is full, so drain it (or drop it).
    pub messages: Receiver<Message>,
    /// Audio chunks from the server. Bounded; newest chunks are dropped when
    /// the consumer falls behind.
    pub audio: Receiver<AudioChunk>,
    /// Artwork chunks from the server. Bounded with drop-on-full.
    pub artwork: Receiver<ArtworkChunk>,
    /// Visualizer chunks from the server. Bounded with drop-on-full.
    pub visualizer: Receiver<VisualizerChunk>,
    /// Clock synchronization state
    pub clock_sync: Arc<Mutex<ClockSync>>,
    /// Sender for writing messages to the server
    pub sender: WsSender,
    /// Controller handle, present when the client declared `controller@v1`.
    /// Commands verify the role is active per the latest `server/activate`.
    pub controller: Option<Controller>,
    /// Source handle, present when the client declared `source@v1`.
    /// Sends verify the role is active per the latest `server/activate`.
    pub source: Option<Source>,
    /// Session facts established during the handshake: `server_id`,
    /// `server_name`, initial `activities` and `active_roles`.
    pub session: SessionInfo,
    /// Must be held alive; dropping aborts background tasks
    pub guard: ConnectionGuard,
}

impl Connection {
    /// See [`WsSender::enter_external_source`].
    pub async fn enter_external_source(&self) -> Result<(), Error> {
        self.sender.enter_external_source().await
    }

    /// See [`WsSender::exit_external_source`].
    pub async fn exit_external_source(
        &self,
        player: Option<crate::protocol::messages::PlayerState>,
    ) -> Result<(), Error> {
        self.sender.exit_external_source(player).await
    }

    /// The live `active_roles` from the latest admissible `server/activate`.
    pub fn active_roles(&self) -> Vec<String> {
        self.sender.shared().active_roles()
    }

    /// The live activities from the latest admissible `server/activate`.
    pub fn activities(&self) -> Vec<Activity> {
        self.sender.shared().activities()
    }
}

/// Aborts background tasks on drop. Hold this alive for the lifetime of the
/// connection.
pub struct ConnectionGuard {
    sender: WsSender,
    router_handle: Option<tokio::task::JoinHandle<()>>,
    sync_handle: Option<tokio::task::JoinHandle<()>>,
    writer_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ConnectionGuard {
    /// Gracefully disconnect: enqueue `client/goodbye`, await the writer's
    /// ack so the goodbye + close frames are known to have flushed (or
    /// surface the wire error if they didn't), then reap the writer.
    ///
    /// Bounded by [`DEFAULT_DISCONNECT_TIMEOUT`]: a peer that stops reading
    /// cannot wedge this call. On deadline the connection is aborted and an
    /// error returned.
    pub async fn disconnect(self, reason: GoodbyeReason) -> Result<(), Error> {
        self.farewell(
            Message::ClientGoodbye(ClientGoodbye { reason }),
            DEFAULT_DISCONNECT_TIMEOUT,
        )
        .await
    }

    /// Send a final message (`client/goodbye` or `pair/abort`) and close,
    /// bounded by `deadline`. On elapse the guard is dropped, aborting the
    /// connection's tasks.
    pub(crate) async fn farewell(mut self, msg: Message, deadline: Duration) -> Result<(), Error> {
        log::debug!("Disconnecting ({msg:?})");
        // Stop clock-sync first so it can't enqueue time samples behind the
        // farewell. The reader stays up until the farewell/close has flushed
        // (below) so the socket isn't half-closed while we're still writing.
        if let Some(h) = self.sync_handle.take() {
            h.abort();
        }

        let flush = async {
            let ack_rx = self.sender.send_farewell(msg)?;
            let result = ack_rx
                .await
                .map_err(|_| Error::WebSocket("connection closed".to_string()))?;
            // Reap the writer separately from awaiting its ack — the ack
            // arrives just before the task returns, so this only joins the
            // trailing teardown.
            if let Some(h) = self.writer_handle.take() {
                let _ = h.await;
            }
            result
        };

        match tokio::time::timeout(deadline, flush).await {
            Ok(result) => {
                // Farewell + close are flushed; tear the reader down now.
                if let Some(h) = self.router_handle.take() {
                    h.abort();
                }
                log::debug!("Disconnect complete");
                result
            }
            Err(_) => {
                // Drop (below) aborts every remaining task.
                log::warn!("Disconnect flush timed out after {deadline:?}; connection aborted");
                Err(Error::Connection(
                    "disconnect flush timed out; connection aborted".to_string(),
                ))
            }
        }
    }

    /// A snapshot of the live facts multi-server arbitration needs: the
    /// current activities and whether a pairing attempt is in progress.
    pub fn arbitration_state(&self) -> ArbitrationState {
        let shared = self.sender.shared();
        ArbitrationState {
            server_id: shared.server_id().to_string(),
            activities: shared.activities(),
            pairing_attempt_in_progress: shared.pairing_attempt_in_progress(),
        }
    }

    /// Resolves once the connection is dead: the router task has exited
    /// (peer close, transport failure, writer failure, or teardown).
    /// Cancel-safe.
    pub(crate) async fn closed(&mut self) {
        if let Some(h) = &mut self.router_handle {
            let _ = h.await;
        }
    }

    /// Non-blocking [`Self::closed`].
    pub(crate) fn is_closed(&self) -> bool {
        self.router_handle
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Some(h) = self.router_handle.take() {
            h.abort();
        }
        if let Some(h) = self.sync_handle.take() {
            h.abort();
        }
        if let Some(h) = self.writer_handle.take() {
            h.abort();
        }
    }
}

/// WebSocket client for Sendspin protocol
pub struct ProtocolClient {
    sender: WsSender,
    audio_rx: Receiver<AudioChunk>,
    artwork_rx: Receiver<ArtworkChunk>,
    visualizer_rx: Receiver<VisualizerChunk>,
    message_rx: Receiver<Message>,
    clock_sync: Arc<Mutex<ClockSync>>,
    session: SessionInfo,
    /// Roles declared in `client/hello`; bounds which typed handles exist.
    declared_roles: Vec<String>,
    /// Background task guard, aborts tasks on drop
    guard: ConnectionGuard,
}

/// Everything `establish` hands to `spawn_session`.
struct EstablishedParts {
    channel: Arc<Mutex<EncryptedChannel>>,
    session: SessionInfo,
    candidate: PskCandidate,
    server_public: [u8; 32],
}

/// Await the next WebSocket text frame during the cleartext handshake phase.
/// Returns the raw text so the caller can hash the exact wire bytes.
async fn next_text_frame<S>(read: &mut SplitStream<WebSocketStream<S>>) -> Result<String, Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let fut = async {
        loop {
            let Some(result) = read.next().await else {
                return Err(Error::Connection(
                    "connection closed during handshake".to_string(),
                ));
            };
            match result {
                Ok(WsMessage::Text(text)) => return Ok(text.to_string()),
                Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => continue,
                Ok(WsMessage::Close(_)) => {
                    return Err(Error::Connection(
                        "server closed connection during handshake".to_string(),
                    ))
                }
                Ok(other) => {
                    return Err(Error::Protocol(format!(
                        "unexpected frame during cleartext handshake: {other:?}"
                    )))
                }
                Err(e) => return Err(Error::WebSocket(e.to_string())),
            }
        }
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
        .await
        .map_err(|_| Error::Connection("handshake timeout".to_string()))?
}

/// Await the next decrypted application JSON message during the encrypted
/// handshake phase (server/hello, server/activate).
async fn next_app_message<S>(
    read: &mut SplitStream<WebSocketStream<S>>,
    channel: &Mutex<EncryptedChannel>,
) -> Result<Message, Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let fut = async {
        loop {
            let Some(result) = read.next().await else {
                return Err(Error::Connection(
                    "connection closed during handshake".to_string(),
                ));
            };
            match result {
                Ok(WsMessage::Binary(data)) => {
                    let decrypted = channel.lock().decrypt_frame(&data)?;
                    match decrypted {
                        None => continue, // fragment in flight
                        Some((frame_type::JSON, payload)) => {
                            return serde_json::from_slice::<Message>(&payload)
                                .map_err(|e| Error::Protocol(e.to_string()))
                        }
                        Some((other, _)) => {
                            return Err(Error::Protocol(format!(
                                "unexpected binary message type {other} before server/activate"
                            )))
                        }
                    }
                }
                Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => continue,
                Ok(WsMessage::Close(_)) => {
                    return Err(Error::Connection(
                        "server closed connection during handshake".to_string(),
                    ))
                }
                Ok(other) => {
                    return Err(Error::Protocol(format!(
                        "unexpected frame on encrypted channel: {other:?}"
                    )))
                }
                Err(e) => return Err(Error::WebSocket(e.to_string())),
            }
        }
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
        .await
        .map_err(|_| Error::Connection("handshake timeout".to_string()))?
}

impl ProtocolClient {
    /// Connect to Sendspin server
    pub(crate) async fn connect<R>(request: R, config: SessionConfig) -> Result<Self, Error>
    where
        R: IntoClientRequest + Unpin,
    {
        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        Self::drive(ws_stream, config).await
    }

    /// Drive the protocol-client state machine over an already-handshaked
    /// WebSocket stream. Shared between outbound `connect()` and inbound
    /// acceptor paths.
    ///
    /// Implements the spec handshake: `client/init` → `server/init` → Noise
    /// messages 1/2 (cleartext text frames), then the encrypted
    /// `server/hello` → `client/hello` → `server/activate` sequence. Any
    /// handshake-phase failure closes the WebSocket without an
    /// application-level error message.
    pub(crate) async fn drive<S>(
        ws_stream: WebSocketStream<S>,
        config: SessionConfig,
    ) -> Result<Self, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut write, mut read) = ws_stream.split();

        match Self::establish(&mut write, &mut read, &config).await {
            Ok(parts) => Self::spawn_session(write, read, config, parts),
            Err(e) => {
                // Handshake failure: close the WebSocket without sending any
                // application-level error message (spec: Failure Handling).
                let _ = write.close().await;
                Err(e)
            }
        }
    }

    /// Run the cleartext + encrypted handshake phases up to (and including)
    /// the initial `server/activate` admission decision.
    async fn establish<S>(
        write: &mut SplitSink<WebSocketStream<S>, WsMessage>,
        read: &mut SplitStream<WebSocketStream<S>>,
        config: &SessionConfig,
    ) -> Result<EstablishedParts, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        // --- Phase 1: cleartext init + Noise handshake (text frames) ---
        let client_init = Message::ClientInit(ClientInit {
            client_id: config.identity.id(),
            version: 1,
            suite: config.suite.wire_name().to_string(),
        });
        let client_init_json =
            serde_json::to_string(&client_init).map_err(|e| Error::Protocol(e.to_string()))?;
        log::debug!("Sending client/init: {}", client_init_json);
        write
            .send(WsMessage::Text(client_init_json.clone().into()))
            .await
            .map_err(|e| Error::WebSocket(e.to_string()))?;

        let server_init_text = next_text_frame(read).await?;
        log::trace!("Received: {}", server_init_text);
        let Message::ServerInit(server_init) = serde_json::from_str::<Message>(&server_init_text)
            .map_err(|e| Error::Protocol(e.to_string()))?
        else {
            return Err(Error::Protocol("expected server/init".to_string()));
        };
        if server_init.version != 1 {
            return Err(Error::Protocol(format!(
                "unsupported server core version {}",
                server_init.version
            )));
        }
        let server_public = b64url_decode_32(&server_init.server_id)?;

        // Prologue: exact wire bytes of client/init followed by server/init.
        let mut prologue = Vec::with_capacity(client_init_json.len() + server_init_text.len());
        prologue.extend_from_slice(client_init_json.as_bytes());
        prologue.extend_from_slice(server_init_text.as_bytes());

        let mut handshake =
            ClientHandshake::new(config.suite, &config.identity, &server_public, &prologue)?;

        // Noise message 1 (server → client): carries the psk_id.
        let msg1_text = next_text_frame(read).await?;
        let Message::NoiseHandshake(msg1) = serde_json::from_str::<Message>(&msg1_text)
            .map_err(|e| Error::Protocol(e.to_string()))?
        else {
            return Err(Error::Protocol("expected noise/handshake".to_string()));
        };
        let msg1_bytes = b64url_decode(&msg1.data)?;
        let payload = handshake.read_message_1(&msg1_bytes)?;
        let psk_payload: NoiseMessage1Payload = serde_json::from_slice(&payload)
            .map_err(|e| Error::Protocol(format!("malformed noise message 1 payload: {e}")))?;

        let candidate = select_psk(&config.psk_candidates, &psk_payload.psk_id)
            .ok_or_else(|| Error::Crypto("psk_id lookup miss".to_string()))?
            .clone();
        // Stored-pubkey model: the matched record must be bound to this server.
        if let PskCategory::LongTerm {
            server_id: Some(bound),
        } = &candidate.category
        {
            if *bound != server_init.server_id {
                return Err(Error::Crypto(
                    "matched PSK is bound to a different server_id".to_string(),
                ));
            }
        }

        // Noise message 2 (client → server): payload is the literal `{}`.
        let msg2 = handshake.write_message_2(&candidate.psk)?;
        let msg2_msg = Message::NoiseHandshake(NoiseHandshake {
            data: b64url_encode(&msg2),
        });
        let msg2_json =
            serde_json::to_string(&msg2_msg).map_err(|e| Error::Protocol(e.to_string()))?;
        write
            .send(WsMessage::Text(msg2_json.into()))
            .await
            .map_err(|e| Error::WebSocket(e.to_string()))?;

        let channel = Arc::new(Mutex::new(handshake.into_channel()?));
        log::debug!(
            "Noise transport established (suite {}, psk category {:?})",
            config.suite.wire_name(),
            candidate.category
        );

        // --- Phase 2: encrypted hello + activate ---
        let Message::ServerHello(server_hello) = next_app_message(read, &channel).await? else {
            return Err(Error::Protocol("expected server/hello".to_string()));
        };
        log::info!(
            "Connected to server: {} ({})",
            server_hello.name,
            server_init.server_id
        );

        let trust_level = trust_level_for(&candidate.category);
        let hello =
            Message::ClientHello(config.hello.to_hello(trust_level, config.unpaired_access));
        send_encrypted(write, &channel, OutboundPayload::Json(Box::new(hello))).await?;

        let Message::ServerActivate(activate) = next_app_message(read, &channel).await? else {
            return Err(Error::Protocol("expected server/activate".to_string()));
        };
        log::debug!("Received server/activate: {:?}", activate);

        let roles = activate.active_roles.clone().unwrap_or_default();
        let pairing_ok = pairing_method_ok(
            &activate,
            &candidate.category,
            &config.hello.supported_pair_methods,
        );
        let verdict = evaluate_activate(
            &candidate.category,
            config.unpaired_access,
            &activate,
            &roles,
            activate.active_roles.is_some(),
            pairing_ok,
        );
        match verdict {
            ActivateVerdict::Admissible => {}
            ActivateVerdict::PairingRequired | ActivateVerdict::Unauthorized => {
                let reason = if verdict == ActivateVerdict::PairingRequired {
                    GoodbyeReason::PairingRequired
                } else {
                    GoodbyeReason::Unauthorized
                };
                log::warn!("server/activate not admissible; closing with {reason:?}");
                let goodbye = Message::ClientGoodbye(ClientGoodbye { reason });
                let _ =
                    send_encrypted(write, &channel, OutboundPayload::Json(Box::new(goodbye))).await;
                let _ = write.close().await;
                return Err(Error::Protocol(
                    "server/activate not admissible".to_string(),
                ));
            }
            ActivateVerdict::MethodNotSupported => {
                // Reply with pair/abort and keep the connection open.
                log::warn!("pairing method not supported; sending pair/abort");
                let abort = Message::PairAbort(PairAbort {
                    reason: PairAbortReason::MethodNotSupported,
                });
                send_encrypted(write, &channel, OutboundPayload::Json(Box::new(abort))).await?;
            }
        }

        let session = SessionInfo {
            server_id: server_init.server_id,
            server_name: server_hello.name,
            trust_level,
            suite: config.suite,
            initial_activities: activate.activities,
            initial_active_roles: roles,
        };
        Ok(EstablishedParts {
            channel,
            session,
            candidate,
            server_public,
        })
    }

    /// Spawn the writer/router/clock-sync tasks for an established session
    /// and enqueue the initial `client/state` (and, for a Pairing PSK
    /// session, the opening `client/pair-finalize`).
    fn spawn_session<S>(
        write: SplitSink<WebSocketStream<S>, WsMessage>,
        read: SplitStream<WebSocketStream<S>>,
        config: SessionConfig,
        parts: EstablishedParts,
    ) -> Result<Self, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let EstablishedParts {
            channel,
            session,
            candidate,
            server_public,
        } = parts;
        let SessionConfig {
            identity,
            suite,
            psk_candidates,
            store,
            pairing_psk,
            record_mode,
            initial_state,
            clock,
            unpaired_access,
            hello,
        } = config;

        // A long-term PSK authenticated this session: mark its record used.
        if matches!(candidate.category, PskCategory::LongTerm { .. }) {
            store.mark_used(&candidate.psk.psk_id());
        }

        let shared = Arc::new(SharedSessionState::new(
            session.server_id.clone(),
            session.initial_activities.clone(),
            session.initial_active_roles.clone(),
        ));
        let (gate_tx, gate_rx) = watch::channel(true);
        let (out_tx, out_rx) = unbounded_channel::<WriteCommand>();
        let (writer_dead_tx, writer_dead_rx) = oneshot::channel();
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_CHANNEL_CAPACITY);
        let (artwork_tx, artwork_rx) = mpsc::channel(ARTWORK_CHANNEL_CAPACITY);
        let (visualizer_tx, visualizer_rx) = mpsc::channel(VISUALIZER_CHANNEL_CAPACITY);
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_CAPACITY);
        let clock_sync = Arc::new(Mutex::new(ClockSync::new(Arc::clone(&clock))));

        let writer_handle = tokio::spawn(writer_task(
            write,
            Arc::clone(&channel),
            out_rx,
            writer_dead_tx,
        ));

        let session_io = SessionIo {
            channel: Arc::clone(&channel),
            out_tx: out_tx.clone(),
            gate: gate_tx,
        };
        let declared_roles = hello.supported_roles.clone();
        let mut session_state = SessionState {
            identity,
            suite,
            server_public,
            server_id: session.server_id.clone(),
            hello,
            management: ManagementState {
                store,
                pairing_psk_enabled: pairing_psk.is_some(),
                pairing_psk,
                unpaired_access,
                record_mode,
            },
            candidates: psk_candidates,
            current: candidate,
            pending_pairing: None,
            pairing_deadline: None,
            activities: session.initial_activities.clone(),
            persisted_roles: session.initial_active_roles.clone(),
            initial_state,
            state_sent: false,
            shared: Arc::clone(&shared),
        };

        // Send initial client/state unless this is a pairing-only session
        // (nothing to report yet). Servers key availability on this even
        // with no active roles, since roles may activate later.
        let pairing_session = session.initial_activities.contains(&Activity::Pairing);
        if !pairing_session {
            log::debug!("Sending initial client/state");
            enqueue_json(
                &out_tx,
                Message::ClientState(session_state.initial_state.clone()),
            );
            session_state.state_sent = true;
        } else if session_state.current.category == PskCategory::Pairing {
            // Pairing PSK flow: the client starts the attempt by delivering a
            // freshly generated long-term PSK immediately after server/activate.
            session_state.start_pairing_attempt(&session_io);
        }

        let clock_sync_router = Arc::clone(&clock_sync);
        let clock_router = Arc::clone(&clock);
        // The router task handle is used by ConnectionGuard::closed() observers.
        let router_handle = tokio::spawn(Self::message_router(
            read,
            RouterChannels {
                channel,
                audio_tx,
                artwork_tx,
                visualizer_tx,
                message_tx,
                clock_sync: clock_sync_router,
                clock: clock_router,
                writer_dead: writer_dead_rx,
            },
            session_state,
            session_io,
        ));

        // First two samples fire 10ms apart so an offset estimate (and
        // playback start) is available almost immediately; drift converges
        // over the following 1Hz samples (see TimeFilter).
        let sender = WsSender::new(out_tx, Arc::clone(&shared), gate_rx);
        let sync_sender = sender.clone();
        let sync_handle = tokio::spawn(async move {
            let mut sample_count: u32 = 0;
            'sync: loop {
                let t1 = clock.now_micros();
                let msg = Message::ClientTime(ClientTime {
                    client_transmitted: t1,
                });
                match sync_sender.send_message(msg).await {
                    Ok(()) => {
                        sample_count = sample_count.saturating_add(1);
                    }
                    Err(e) => {
                        log::info!("Clock sync task exiting: {}", e);
                        break 'sync;
                    }
                }
                let delay = if sample_count < 2 {
                    tokio::time::Duration::from_millis(10)
                } else {
                    tokio::time::Duration::from_secs(1)
                };
                tokio::time::sleep(delay).await;
            }
        });

        Ok(Self {
            sender: sender.clone(),
            audio_rx,
            artwork_rx,
            visualizer_rx,
            message_rx,
            clock_sync,
            session,
            declared_roles,
            guard: ConnectionGuard {
                sender,
                router_handle: Some(router_handle),
                sync_handle: Some(sync_handle),
                writer_handle: Some(writer_handle),
            },
        })
    }

    async fn message_router<S>(
        mut read: SplitStream<WebSocketStream<S>>,
        mut io: RouterChannels,
        mut session: SessionState,
        session_io: SessionIo,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let mut audio_closed = false;
        let mut artwork_closed = false;
        let mut visualizer_closed = false;
        let mut message_closed = false;
        let mut audio_chunk_count = 0u64;
        let mut audio_dropped_count = 0u64;
        let mut visualizer_chunk_count = 0u64;

        'outer: loop {
            // A pairing attempt is bounded by the spec attempt timeout.
            let pairing_deadline = session.pairing_deadline;
            let frame = tokio::select! {
                biased;
                // The writer half died (wire-write failure): the session is
                // over even if the read half still looks idle-healthy.
                _ = &mut io.writer_dead => {
                    log::info!("Writer task ended; closing session");
                    break 'outer;
                }
                _ = tokio::time::sleep_until(
                    pairing_deadline.unwrap_or_else(far_future)
                ), if pairing_deadline.is_some() => {
                    session.abort_pairing_attempt_timeout(&session_io);
                    continue;
                }
                frame = read.next() => frame,
            };
            let Some(msg) = frame else { break };
            match msg {
                Ok(WsMessage::Binary(data)) => {
                    // Capture receive time before decryption so t4 is as
                    // close to the true arrival time as possible.
                    let t4 = io.clock.now_micros();
                    let decrypted = match io.channel.lock().decrypt_frame(&data) {
                        Ok(d) => d,
                        Err(e) => {
                            // AEAD failure or malformed fragment sequence:
                            // protocol error, close the connection.
                            log::error!("Transport decrypt failed: {e}; closing connection");
                            break;
                        }
                    };
                    let Some((msg_type, payload)) = decrypted else {
                        continue; // fragment in flight
                    };
                    if msg_type == frame_type::JSON {
                        match serde_json::from_slice::<Message>(&payload) {
                            Ok(msg) => {
                                // ServerTime is consumed here for clock sync
                                // and intentionally NOT forwarded to
                                // message_rx consumers — it's an internal
                                // protocol detail arriving at 1Hz.
                                if let Message::ServerTime(ref st) = msg {
                                    io.clock_sync.lock().update(
                                        st.client_transmitted,
                                        st.server_received,
                                        st.server_transmitted,
                                        t4,
                                    );
                                    continue;
                                }
                                log::debug!("Received message: {:?}", msg);
                                match session.handle_json(&msg, &session_io) {
                                    SessionFlow::Close => break 'outer,
                                    SessionFlow::Consumed => continue,
                                    SessionFlow::Forward => {}
                                }
                                // Bounded forward: a consumer that never
                                // drains control messages stalls the router
                                // (and the connection) instead of growing
                                // memory without bound.
                                if !message_closed && io.message_tx.send(msg).await.is_err() {
                                    log::error!(
                                        "Message receiver dropped — messages will be discarded"
                                    );
                                    message_closed = true;
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to parse message: {}", e);
                            }
                        }
                        continue;
                    }
                    // Binary role data: reconstruct [type][payload] framing.
                    match BinaryFrame::from_parts(msg_type, &payload) {
                        Ok(BinaryFrame::Audio(chunk)) => {
                            audio_chunk_count += 1;
                            if should_log_sample(audio_chunk_count) {
                                log::trace!(
                                    "Received audio chunk: chunk={}, timestamp={}µs, payload_bytes={}, wire_bytes={}",
                                    audio_chunk_count,
                                    chunk.timestamp,
                                    chunk.data.len(),
                                    data.len()
                                );
                            }
                            if !audio_closed {
                                match io.audio_tx.try_send(chunk) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        audio_dropped_count += 1;
                                        if should_log_sample(audio_dropped_count) {
                                            log::warn!(
                                                "Audio receiver falling behind — dropped {} chunks",
                                                audio_dropped_count
                                            );
                                        }
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {
                                        log::error!(
                                            "Audio receiver dropped — audio data will be discarded"
                                        );
                                        audio_closed = true;
                                    }
                                }
                            }
                        }
                        Ok(BinaryFrame::Artwork(chunk)) => {
                            // Artwork arrives in short bursts on track
                            // changes, so every chunk is worth a line; audio
                            // and visualizer chunks stream continuously and
                            // are sampled instead.
                            log::trace!(
                                "Received artwork chunk: channel={}, timestamp={}µs, payload_bytes={}",
                                chunk.channel,
                                chunk.timestamp,
                                chunk.data.len()
                            );
                            if !artwork_closed {
                                match io.artwork_tx.try_send(chunk) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        log::warn!("Artwork receiver full — dropping chunk");
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {
                                        log::error!(
                                            "Artwork receiver dropped — artwork data will be discarded"
                                        );
                                        artwork_closed = true;
                                    }
                                }
                            }
                        }
                        Ok(BinaryFrame::Visualizer(chunk)) => {
                            visualizer_chunk_count += 1;
                            if should_log_sample(visualizer_chunk_count) {
                                log::trace!(
                                    "Received visualizer chunk: chunk={}, timestamp={}µs, payload_bytes={}",
                                    visualizer_chunk_count,
                                    chunk.timestamp,
                                    chunk.data.len()
                                );
                            }
                            if !visualizer_closed {
                                match io.visualizer_tx.try_send(chunk) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {}
                                    Err(mpsc::error::TrySendError::Closed(_)) => {
                                        log::error!(
                                            "Visualizer receiver dropped — visualizer data will be discarded"
                                        );
                                        visualizer_closed = true;
                                    }
                                }
                            }
                        }
                        Ok(BinaryFrame::Unknown { type_id, .. }) => {
                            log::warn!("Received unknown binary type: {}", type_id);
                        }
                        Err(e) => {
                            log::warn!("Failed to parse binary frame: {}", e);
                        }
                    }
                }
                Ok(WsMessage::Text(text)) => {
                    // After the handshake, all Sendspin messages are
                    // encrypted binary frames; a text frame is a protocol
                    // violation.
                    log::error!(
                        "Unexpected text frame on encrypted channel ({} bytes); closing",
                        text.len()
                    );
                    break;
                }
                Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)) => {}
                Ok(WsMessage::Close(_)) => {
                    log::info!("Server closed connection");
                    break;
                }
                Err(e) => {
                    log::error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
        log::debug!("Message router: WebSocket stream ended");
    }

    /// Gracefully disconnect: sends `client/goodbye`, closes the WebSocket,
    /// and aborts background tasks. Bounded by
    /// [`DEFAULT_DISCONNECT_TIMEOUT`].
    pub async fn disconnect(self, reason: GoodbyeReason) -> Result<(), Error> {
        self.guard.disconnect(reason).await
    }

    /// See [`WsSender::enter_external_source`].
    pub async fn enter_external_source(&self) -> Result<(), Error> {
        self.sender.enter_external_source().await
    }

    /// See [`WsSender::exit_external_source`].
    pub async fn exit_external_source(
        &self,
        player: Option<crate::protocol::messages::PlayerState>,
    ) -> Result<(), Error> {
        self.sender.exit_external_source(player).await
    }

    /// Get reference to clock sync
    pub fn clock_sync(&self) -> Arc<Mutex<ClockSync>> {
        Arc::clone(&self.clock_sync)
    }

    /// Session facts established during the handshake: `server_id`,
    /// `server_name`, and the initial `activities` / `active_roles`.
    pub fn session(&self) -> &SessionInfo {
        &self.session
    }

    /// The live `active_roles` from the latest admissible `server/activate`.
    pub fn active_roles(&self) -> Vec<String> {
        self.sender.shared().active_roles()
    }

    /// Split into separate receivers for concurrent processing.
    ///
    /// This allows using `tokio::select!` to process messages and binary
    /// data concurrently. Use the fields you need; ignore the rest.
    pub fn split(self) -> Connection {
        let sender = self.sender;
        let declared = |role: &str| self.declared_roles.iter().any(|r| r == role);
        let controller = declared("controller@v1").then(|| Controller::new(sender.clone()));
        let source = declared("source@v1").then(|| Source::new(sender.clone()));
        Connection {
            messages: self.message_rx,
            audio: self.audio_rx,
            artwork: self.artwork_rx,
            visualizer: self.visualizer_rx,
            clock_sync: self.clock_sync,
            sender,
            controller,
            source,
            session: self.session,
            guard: self.guard,
        }
    }
}

/// Channels and clocks the router routes into.
struct RouterChannels {
    channel: Arc<Mutex<EncryptedChannel>>,
    audio_tx: Sender<AudioChunk>,
    artwork_tx: Sender<ArtworkChunk>,
    visualizer_tx: Sender<VisualizerChunk>,
    message_tx: Sender<Message>,
    clock_sync: Arc<Mutex<ClockSync>>,
    clock: Arc<dyn Clock>,
    writer_dead: oneshot::Receiver<()>,
}

/// A `tokio::time::Instant` far enough away to stand in for "no deadline".
fn far_future() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(86400)
}
