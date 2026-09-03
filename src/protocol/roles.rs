// ABOUTME: Role facades (WsSender, Controller, Source) and live shared session state
// ABOUTME: Senders are gated during re-handshakes and check live active_roles per call

//! Application-facing senders for an established connection.
//!
//! [`WsSender`] is the low-level message sender; [`Controller`] and
//! [`Source`] are typed facades over it that verify their role is currently
//! active (per the latest `server/activate`) before sending.

use crate::error::Error;
use crate::protocol::binary::binary_types;
use crate::protocol::messages::{
    Activity, ArtworkState, AudioFormatSpec, ClientCommand, ClientState, ClientStreamEnd,
    ClientStreamStart, ControllerCommand, ControllerCommandType, Message, PlayerState, RepeatMode,
    SourceStreamConfig, StreamEnd, StreamStart, VisualizerState,
};
use crate::protocol::writer::{OutboundPayload, WriteCommand};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

/// Bare role names as they appear in `stream/end` role lists — distinct from
/// the versioned `player@v1` names used during role negotiation.
const ROLE_PLAYER: &str = "player";
const ROLE_ARTWORK: &str = "artwork";
const ROLE_VISUALIZER: &str = "visualizer";

/// Which role streams are currently active, updated by the message router from
/// `stream/start` and `stream/end`.
#[derive(Debug, Default)]
pub(crate) struct StreamState {
    player_active: AtomicBool,
    artwork_active: AtomicBool,
    visualizer_active: AtomicBool,
}

impl StreamState {
    /// A `stream/start` for one role must not disturb another's stream, so
    /// absent roles are left untouched rather than cleared.
    pub(crate) fn note_stream_start(&self, start: &StreamStart) {
        if start.player.is_some() {
            self.player_active.store(true, Ordering::Release);
        }
        if start.artwork.is_some() {
            self.artwork_active.store(true, Ordering::Release);
        }
        if start.visualizer.is_some() {
            self.visualizer_active.store(true, Ordering::Release);
        }
    }

    /// `stream/end` with no roles ends every stream; otherwise only those listed.
    pub(crate) fn note_stream_end(&self, end: &StreamEnd) {
        if role_ended(end, ROLE_PLAYER) {
            self.player_active.store(false, Ordering::Release);
        }
        if role_ended(end, ROLE_ARTWORK) {
            self.artwork_active.store(false, Ordering::Release);
        }
        if role_ended(end, ROLE_VISUALIZER) {
            self.visualizer_active.store(false, Ordering::Release);
        }
    }
}

fn role_ended(end: &StreamEnd, role: &str) -> bool {
    end.roles
        .as_ref()
        .is_none_or(|roles| roles.iter().any(|r| r == role))
}

/// Availability request and clock-sync state. Both transitions are serialized
/// with the canonical client-state update by `SharedSessionState`.
#[derive(Debug)]
struct AvailabilityState {
    requested: bool,
    synchronized: bool,
}

/// Live, mutable session facts maintained by the message router.
#[derive(Debug, Default)]
struct LiveSession {
    activities: Vec<Activity>,
    active_roles: Vec<String>,
    pairing_attempt: bool,
}

/// State shared between the message router (writer of truth) and the
/// application-facing handles (readers): the current activities and
/// `active_roles` from the latest admissible `server/activate`, stream
/// activity, and whether a pairing attempt is in progress.
#[derive(Debug)]
pub(crate) struct SharedSessionState {
    server_id: String,
    stream: StreamState,
    live: RwLock<LiveSession>,
    /// The canonical client state: every `client/state` this library sends
    /// carries full role objects drawn from (and recorded into) this copy.
    client_state: RwLock<ClientState>,
    /// Availability request and synchronization state, serialized with the
    /// canonical client-state transition.
    availability: RwLock<AvailabilityState>,
    /// Whether a player role requires clock synchronization before availability.
    requires_clock_sync: bool,
    /// Formats declared by the player role in `client/hello`.
    supported_formats: Vec<AudioFormatSpec>,
}

impl SharedSessionState {
    pub(crate) fn new(
        server_id: String,
        activities: Vec<Activity>,
        roles: Vec<String>,
        client_state: ClientState,
        requested_available: bool,
        requires_clock_sync: bool,
        supported_formats: Vec<AudioFormatSpec>,
    ) -> Self {
        Self {
            server_id,
            stream: StreamState::default(),
            live: RwLock::new(LiveSession {
                activities,
                active_roles: roles,
                pairing_attempt: false,
            }),
            client_state: RwLock::new(client_state),
            availability: RwLock::new(AvailabilityState {
                requested: requested_available,
                synchronized: false,
            }),
            requires_clock_sync,
            supported_formats,
        }
    }

    /// A snapshot of the canonical client state.
    pub(crate) fn client_state(&self) -> ClientState {
        self.client_state.read().clone()
    }

    /// Mutate the canonical client state and return the updated snapshot.
    pub(crate) fn update_client_state(&self, f: impl FnOnce(&mut ClientState)) -> ClientState {
        let mut state = self.client_state.write();
        f(&mut state);
        state.clone()
    }

    pub(crate) fn set_clock_synchronized(&self) -> bool {
        let mut availability = self.availability.write();
        if availability.synchronized {
            return false;
        }
        availability.synchronized = true;
        let effective = availability.requested;
        if self.requires_clock_sync && effective {
            self.client_state.write().available = true;
            return true;
        }
        false
    }

    pub(crate) fn update_available_state(
        &self,
        requested: bool,
        update: impl FnOnce(&mut ClientState),
    ) -> ClientState {
        let mut availability = self.availability.write();
        availability.requested = requested;
        let effective = requested && (!self.requires_clock_sync || availability.synchronized);
        let mut state = self.client_state.write();
        update(&mut state);
        state.available = effective;
        state.clone()
    }

    pub(crate) fn format_supported(&self, format: &AudioFormatSpec) -> bool {
        self.supported_formats
            .iter()
            .any(|supported| supported == format)
    }

    #[cfg(test)]
    fn availability_state(&self) -> (bool, bool, bool) {
        let availability = self.availability.read();
        (
            availability.requested,
            availability.synchronized,
            self.client_state.read().available,
        )
    }

    pub(crate) fn server_id(&self) -> &str {
        &self.server_id
    }

    pub(crate) fn stream(&self) -> &StreamState {
        &self.stream
    }

    pub(crate) fn has_role(&self, role: &str) -> bool {
        self.live.read().active_roles.iter().any(|r| r == role)
    }

    pub(crate) fn active_roles(&self) -> Vec<String> {
        self.live.read().active_roles.clone()
    }

    pub(crate) fn activities(&self) -> Vec<Activity> {
        self.live.read().activities.clone()
    }

    pub(crate) fn pairing_attempt_in_progress(&self) -> bool {
        self.live.read().pairing_attempt
    }

    pub(crate) fn set_activation(&self, activities: Vec<Activity>, roles: Vec<String>) {
        let mut live = self.live.write();
        live.activities = activities;
        live.active_roles = roles;
    }

    pub(crate) fn set_pairing_attempt(&self, in_progress: bool) {
        self.live.write().pairing_attempt = in_progress;
    }
}

/// Cheap to clone. `send_message` returns once the writer has reported the
/// underlying `sink.send` result, so the `Result` reflects the wire-write
/// outcome rather than queue insertion.
///
/// During an in-band re-handshake, sends wait until the post-re-handshake
/// `server/activate` arrives (spec: no other messages flow during the
/// exchange, and the connection resumes with the hello/activate sequence).
#[derive(Debug, Clone)]
pub struct WsSender {
    tx: UnboundedSender<WriteCommand>,
    /// The router updates this *before* forwarding the triggering `stream/start`
    /// / `stream/end`, so a consumer that reacts to those messages already
    /// observes the settled state.
    shared: Arc<SharedSessionState>,
    /// Open (`true`) in normal operation; closed by the router between
    /// re-handshake initiation and the next `server/activate`.
    gate: watch::Receiver<bool>,
}

impl WsSender {
    pub(crate) fn new(
        tx: UnboundedSender<WriteCommand>,
        shared: Arc<SharedSessionState>,
        gate: watch::Receiver<bool>,
    ) -> Self {
        Self { tx, shared, gate }
    }

    pub(crate) fn shared(&self) -> &Arc<SharedSessionState> {
        &self.shared
    }

    /// Wait until the write gate is open. Fails when the session is gone
    /// (the router, which owns the gate, has exited).
    async fn wait_gate(&self) -> Result<(), Error> {
        // Fast path: gate open.
        if *self.gate.borrow() {
            return Ok(());
        }
        let mut gate = self.gate.clone();
        gate.wait_for(|open| *open)
            .await
            .map(|_| ())
            .map_err(|_| Error::WebSocket("connection closed".to_string()))
    }

    /// Send a message to the server.
    pub async fn send_message(&self, msg: Message) -> Result<(), Error> {
        self.wait_gate().await?;
        // Time pings go out at 1Hz for as long as the connection lives; keep
        // that housekeeping at trace so debug shows only meaningful traffic.
        let level = if matches!(msg, Message::ClientTime(_)) {
            log::Level::Trace
        } else {
            log::Level::Debug
        };
        if log::log_enabled!(level) {
            if let Ok(json) = serde_json::to_string(&msg) {
                log::log!(level, "Sending message: {}", json);
            }
        }

        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteCommand::Send {
                msg: OutboundPayload::Json(Box::new(msg)),
                ack: ack_tx,
            })
            .map_err(|_| Error::WebSocket("connection closed".to_string()))?;

        // A cancelled ack means the writer dropped the command unsent — the
        // connection is gone either way.
        ack_rx
            .await
            .map_err(|_| Error::WebSocket("connection closed".to_string()))?
    }

    /// Send a raw binary application message (e.g. source audio chunks,
    /// binary ID 12). The payload excludes the leading message-type byte.
    pub async fn send_binary(&self, msg_type: u8, payload: Vec<u8>) -> Result<(), Error> {
        self.wait_gate().await?;
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteCommand::Send {
                msg: OutboundPayload::Binary { msg_type, payload },
                ack: ack_tx,
            })
            .map_err(|_| Error::WebSocket("connection closed".to_string()))?;
        ack_rx
            .await
            .map_err(|_| Error::WebSocket("connection closed".to_string()))?
    }

    /// Send a top-level availability update.
    pub async fn send_available(&self, available: bool) -> Result<(), Error> {
        let snapshot = self.shared.update_available_state(available, |_| {});
        self.send_message(Message::ClientState(ClientState {
            available: snapshot.available,
            ..Default::default()
        }))
        .await
    }

    /// Tell the server this client is temporarily owned by another audio source
    /// (`available: false`).
    ///
    /// Release any Sendspin-owned output first so the external source can open
    /// the device without racing this client's audio stream.
    pub async fn enter_external_source(&self) -> Result<(), Error> {
        self.send_available(false).await
    }

    /// Tell the server this client is available for Sendspin playback again
    /// (`available: true`).
    ///
    /// Include player state when volume, mute, or output delay may have changed
    /// while the external source owned the device. Hardware/OS mixer changes
    /// must be read through platform APIs; this library only tracks its own
    /// software [`GainControl`](crate::audio::GainControl).
    pub async fn exit_external_source(&self, player: Option<PlayerState>) -> Result<(), Error> {
        if let Some(player) = player.as_ref() {
            player
                .validate()
                .map_err(|message| Error::Protocol(message.to_string()))?;
            if player
                .format
                .as_ref()
                .is_some_and(|format| !self.shared.format_supported(format))
            {
                return Err(Error::Protocol(
                    "player format is not in supported_formats".to_string(),
                ));
            }
        }
        let snapshot = self.shared.update_available_state(true, |state| {
            if let Some(player) = player {
                state.player = Some(player);
            }
        });
        self.send_message(Message::ClientState(ClientState {
            available: snapshot.available,
            player: snapshot.player,
            ..Default::default()
        }))
        .await
    }

    /// Report a full player state update (`client/state` player object).
    ///
    /// Stream configuration is derived from client state: when the player's
    /// `format` preference changes while a player stream is active, the
    /// server re-derives the stream format and re-issues `stream/start` if it
    /// changed; with no active stream, the preference applies to the next
    /// stream. Timing fields (`required_lead_time_ms`, `min_buffer_ms`,
    /// `output_delay_ms`) feed the server's send-ahead planning.
    pub async fn update_player_state(&self, player: PlayerState) -> Result<(), Error> {
        player
            .validate()
            .map_err(|message| Error::Protocol(message.to_string()))?;
        if player
            .format
            .as_ref()
            .is_some_and(|format| !self.shared.format_supported(format))
        {
            return Err(Error::Protocol(
                "player format is not in supported_formats".to_string(),
            ));
        }
        let snapshot = self
            .shared
            .update_client_state(|state| state.player = Some(player));
        self.send_client_state_roles(ClientState {
            player: snapshot.player,
            ..Default::default()
        })
        .await
    }

    /// Change (or clear) the player's preferred audio format.
    ///
    /// The format must be one of the `supported_formats` declared in
    /// `client/hello`; `None` restores the server's priority-order selection.
    /// Requires a player object to have been reported (the builder seeds one
    /// for player clients).
    pub async fn set_player_format(&self, format: Option<AudioFormatSpec>) -> Result<(), Error> {
        if format
            .as_ref()
            .is_some_and(|format| !self.shared.format_supported(format))
        {
            return Err(Error::Protocol(
                "player format is not in supported_formats".to_string(),
            ));
        }
        let snapshot = self.shared.update_client_state(|state| {
            if let Some(player) = state.player.as_mut() {
                player.format = format;
            }
        });
        let Some(player) = snapshot.player else {
            return Err(Error::Protocol(
                "no player state to carry a format preference".to_string(),
            ));
        };
        self.send_client_state_roles(ClientState {
            player: Some(player),
            ..Default::default()
        })
        .await
    }

    /// Report a full artwork channel configuration (`client/state` artwork
    /// object). Channels are positional from channel 0 (at most 4); a channel
    /// the array does not cover is `source: 'none'`.
    pub async fn update_artwork_state(&self, artwork: ArtworkState) -> Result<(), Error> {
        artwork
            .validate()
            .map_err(|message| Error::Protocol(message.to_string()))?;
        let snapshot = self
            .shared
            .update_client_state(|state| state.artwork = Some(artwork));
        self.send_client_state_roles(ClientState {
            artwork: snapshot.artwork,
            ..Default::default()
        })
        .await
    }

    /// Report a full visualizer configuration (`client/state` visualizer
    /// object): requested data types, frame-rate cap, and spectrum layout.
    pub async fn update_visualizer_state(&self, visualizer: VisualizerState) -> Result<(), Error> {
        visualizer
            .validate()
            .map_err(|message| Error::Protocol(message.to_string()))?;
        let snapshot = self
            .shared
            .update_client_state(|state| state.visualizer = Some(visualizer));
        self.send_client_state_roles(ClientState {
            visualizer: snapshot.visualizer,
            ..Default::default()
        })
        .await
    }

    /// Send a `client/state` carrying the given role objects plus the
    /// canonical `available` flag.
    async fn send_client_state_roles(&self, mut state: ClientState) -> Result<(), Error> {
        state.available = self.shared.client_state().available;
        self.send_message(Message::ClientState(state)).await
    }

    /// Enqueue a farewell message (goodbye or pair/abort) followed by a
    /// WebSocket close. Bypasses the re-handshake gate: teardown must not
    /// deadlock behind a stalled re-handshake.
    pub(crate) fn send_farewell(
        &self,
        msg: Message,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<(), Error>>, Error> {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteCommand::Farewell {
                msg: Box::new(msg),
                ack: ack_tx,
            })
            .map_err(|_| Error::WebSocket("connection closed".to_string()))?;
        Ok(ack_rx)
    }
}

/// Controller handle for sending playback commands to the server.
///
/// Present when the client declared the `controller@v1` role in
/// `client/hello`; each command verifies the role is currently active (per
/// the latest `server/activate`) and fails with a protocol error otherwise.
/// Obtained via [`ProtocolClient::split()`](crate::ProtocolClient::split).
#[derive(Debug, Clone)]
pub struct Controller {
    sender: WsSender,
}

impl Controller {
    pub(crate) fn new(sender: WsSender) -> Self {
        Self { sender }
    }

    async fn send_controller_command(&self, cmd: ControllerCommand) -> Result<(), Error> {
        if !self.sender.shared().has_role("controller@v1") {
            return Err(Error::Protocol(
                "controller@v1 role is not active on this connection".to_string(),
            ));
        }
        let msg = Message::ClientCommand(ClientCommand {
            controller: Some(cmd),
        });
        self.sender.send_message(msg).await
    }

    async fn send_simple_command(&self, command: ControllerCommandType) -> Result<(), Error> {
        self.send_controller_command(ControllerCommand {
            command,
            volume: None,
            mute: None,
            position_ms: None,
            offset_ms: None,
        })
        .await
    }

    /// Resume playback
    pub async fn play(&self) -> Result<(), Error> {
        self.send_simple_command(ControllerCommandType::Play).await
    }

    /// Pause playback
    pub async fn pause(&self) -> Result<(), Error> {
        self.send_simple_command(ControllerCommandType::Pause).await
    }

    /// Stop playback
    pub async fn stop(&self) -> Result<(), Error> {
        self.send_simple_command(ControllerCommandType::Stop).await
    }

    /// Skip to next track
    pub async fn next(&self) -> Result<(), Error> {
        self.send_simple_command(ControllerCommandType::Next).await
    }

    /// Skip to previous track
    pub async fn previous(&self) -> Result<(), Error> {
        self.send_simple_command(ControllerCommandType::Previous)
            .await
    }

    /// Set group volume (0-100). Values above 100 are clamped.
    pub async fn set_volume(&self, volume: u8) -> Result<(), Error> {
        self.send_controller_command(ControllerCommand {
            command: ControllerCommandType::Volume,
            volume: Some(volume.clamp(0, 100)),
            mute: None,
            position_ms: None,
            offset_ms: None,
        })
        .await
    }

    /// Set group mute state
    pub async fn set_mute(&self, muted: bool) -> Result<(), Error> {
        self.send_controller_command(ControllerCommand {
            command: ControllerCommandType::Mute,
            volume: None,
            mute: Some(muted),
            position_ms: None,
            offset_ms: None,
        })
        .await
    }

    /// Set repeat mode
    pub async fn repeat(&self, mode: RepeatMode) -> Result<(), Error> {
        let command = match mode {
            RepeatMode::Off => ControllerCommandType::RepeatOff,
            RepeatMode::One => ControllerCommandType::RepeatOne,
            RepeatMode::All => ControllerCommandType::RepeatAll,
        };
        self.send_simple_command(command).await
    }

    /// Enable or disable shuffle
    pub async fn shuffle(&self, enabled: bool) -> Result<(), Error> {
        let command = if enabled {
            ControllerCommandType::Shuffle
        } else {
            ControllerCommandType::Unshuffle
        };
        self.send_simple_command(command).await
    }

    /// Switch to next group
    pub async fn switch(&self) -> Result<(), Error> {
        self.send_simple_command(ControllerCommandType::Switch)
            .await
    }

    /// Seek to an absolute playback position in milliseconds.
    ///
    /// Only send this when `seek` is in the server's `supported_commands`.
    /// Per the spec, the server ignores the command if `position_ms` is
    /// outside the range 0 to
    /// [`ControllerState::seek_max_ms`](crate::protocol::messages::ControllerState::seek_max_ms).
    pub async fn seek(&self, position_ms: u64) -> Result<(), Error> {
        self.send_controller_command(ControllerCommand {
            command: ControllerCommandType::Seek,
            volume: None,
            mute: None,
            position_ms: Some(position_ms),
            offset_ms: None,
        })
        .await
    }

    /// Seek by a signed offset in milliseconds from the current position
    /// (positive forward, negative backward).
    ///
    /// Only send this when `seek_relative` is in the server's
    /// `supported_commands`. The server applies the offset on a best-effort
    /// basis and clamps the result to the seekable range.
    pub async fn seek_relative(&self, offset_ms: i64) -> Result<(), Error> {
        self.send_controller_command(ControllerCommand {
            command: ControllerCommandType::SeekRelative,
            volume: None,
            mute: None,
            position_ms: None,
            offset_ms: Some(offset_ms),
        })
        .await
    }
}

/// Source handle for streaming captured audio to the server.
///
/// Present when the client declared the `source@v1` role in `client/hello`;
/// each send verifies the role is currently active. Obtained via
/// [`ProtocolClient::split()`](crate::ProtocolClient::split).
///
/// The server controls streaming: capture and send chunks only after a
/// `server/command` `source` object with `command: start`, and stop (ending
/// the stream) on `command: stop`. Those commands arrive on the
/// [`Connection::messages`](crate::Connection) channel.
#[derive(Debug, Clone)]
pub struct Source {
    sender: WsSender,
}

impl Source {
    pub(crate) fn new(sender: WsSender) -> Self {
        Self { sender }
    }

    fn require_role(&self) -> Result<(), Error> {
        if !self.sender.shared().has_role("source@v1") {
            return Err(Error::Protocol(
                "source@v1 role is not active on this connection".to_string(),
            ));
        }
        Ok(())
    }

    /// Announce the active input stream format (`client-stream/start`).
    /// Must be sent before the first audio chunk; re-sending replaces the
    /// format in place.
    pub async fn start_stream(&self, config: SourceStreamConfig) -> Result<(), Error> {
        self.require_role()?;
        self.sender
            .send_message(Message::ClientStreamStart(ClientStreamStart {
                source: config,
            }))
            .await
    }

    /// End the current input stream (`client-stream/end`).
    pub async fn end_stream(&self) -> Result<(), Error> {
        self.require_role()?;
        self.sender
            .send_message(Message::ClientStreamEnd(ClientStreamEnd {}))
            .await
    }

    /// Send one source audio chunk (binary ID 12).
    ///
    /// `capture_timestamp_us` is the server-domain time the first sample was
    /// captured (invert the time filter's mapping to produce it). Chunks must
    /// carry whole codec units, be at most 150 ms long, and SHOULD be at
    /// least 5 ms (the final chunk before `client-stream/end` may be shorter).
    pub async fn send_chunk(&self, capture_timestamp_us: i64, data: &[u8]) -> Result<(), Error> {
        self.require_role()?;
        let mut payload = Vec::with_capacity(8 + data.len());
        payload.extend_from_slice(&capture_timestamp_us.to_be_bytes());
        payload.extend_from_slice(data);
        self.sender
            .send_binary(binary_types::SOURCE_AUDIO, payload)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_request_after_sync_updates_canonical_state_atomically() {
        let state = SharedSessionState::new(
            "server".to_string(),
            Vec::new(),
            Vec::new(),
            ClientState::default(),
            true,
            true,
            Vec::new(),
        );

        assert!(state.set_clock_synchronized());
        assert_eq!(state.availability_state(), (true, true, true));
        let snapshot = state.update_available_state(false, |_| {});
        assert!(!snapshot.available);
        assert_eq!(state.availability_state(), (false, true, false));
        let snapshot = state.update_available_state(true, |_| {});
        assert!(snapshot.available);
        assert_eq!(state.availability_state(), (true, true, true));
    }

    #[test]
    fn external_source_rejects_unsupported_player_format() {
        let state = Arc::new(SharedSessionState::new(
            "server".to_string(),
            Vec::new(),
            Vec::new(),
            ClientState::default(),
            true,
            false,
            vec![AudioFormatSpec {
                codec: "pcm".to_string(),
                channels: 2,
                sample_rate: 48_000,
                bit_depth: 16,
            }],
        ));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_gate_tx, gate_rx) = watch::channel(true);
        let sender = WsSender::new(tx, state, gate_rx);
        let unsupported = PlayerState {
            format: Some(AudioFormatSpec {
                codec: "opus".to_string(),
                channels: 2,
                sample_rate: 48_000,
                bit_depth: 16,
            }),
            ..Default::default()
        };

        let result = tokio_test::block_on(sender.exit_external_source(Some(unsupported)));
        assert!(
            matches!(result, Err(Error::Protocol(message)) if message.contains("supported_formats"))
        );
    }
}
