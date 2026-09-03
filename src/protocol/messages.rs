// ABOUTME: Protocol message type definitions and serialization
// ABOUTME: Supports all Sendspin protocol messages per spec

use serde::{Deserialize, Serialize};

/// Top-level protocol message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum Message {
    // === Cleartext handshake messages (WebSocket text frames) ===
    /// First message sent by the client after the WebSocket opens
    #[serde(rename = "client/init")]
    ClientInit(ClientInit),

    /// Server response to `client/init`
    #[serde(rename = "server/init")]
    ServerInit(ServerInit),

    /// One Noise handshake message (also used for in-band re-handshakes)
    #[serde(rename = "noise/handshake")]
    NoiseHandshake(NoiseHandshake),

    // === Encrypted handshake messages ===
    /// First message from the server after the Noise handshake completes
    #[serde(rename = "server/hello")]
    ServerHello(ServerHello),

    /// Client capabilities and roles, sent after `server/hello`
    #[serde(rename = "client/hello")]
    ClientHello(ClientHello),

    /// Server's declared purpose on this connection (activities/roles)
    #[serde(rename = "server/activate")]
    ServerActivate(ServerActivate),

    // === Time synchronization ===
    /// Client time synchronization request
    #[serde(rename = "client/time")]
    ClientTime(ClientTime),

    /// Server time synchronization response
    #[serde(rename = "server/time")]
    ServerTime(ServerTime),

    // === State messages ===
    /// Client state update
    #[serde(rename = "client/state")]
    ClientState(ClientState),

    /// Server state update (metadata, controller info)
    #[serde(rename = "server/state")]
    ServerState(ServerState),

    // === Command messages ===
    /// Server command to client (player commands)
    #[serde(rename = "server/command")]
    ServerCommand(ServerCommand),

    /// Client command to server (controller commands)
    #[serde(rename = "client/command")]
    ClientCommand(ClientCommand),

    // === Stream control messages ===
    /// Stream start notification
    #[serde(rename = "stream/start")]
    StreamStart(StreamStart),

    /// Stream end notification
    #[serde(rename = "stream/end")]
    StreamEnd(StreamEnd),

    /// Stream clear notification
    #[serde(rename = "stream/clear")]
    StreamClear(StreamClear),

    // === Source (client → server) stream messages ===
    /// Source input stream start (format announcement)
    #[serde(rename = "client-stream/start")]
    ClientStreamStart(ClientStreamStart),

    /// Source input stream end
    #[serde(rename = "client-stream/end")]
    ClientStreamEnd(ClientStreamEnd),

    // === Group messages ===
    /// Group update notification
    #[serde(rename = "group/update")]
    GroupUpdate(GroupUpdate),

    // === Pairing messages ===
    /// Client reports a gesture-gated attempt awaiting a pairing window
    #[serde(rename = "client/pair-pending")]
    ClientPairPending(ClientPairPending),

    /// Client starts a code-based pairing attempt
    #[serde(rename = "client/pair-init")]
    ClientPairInit(ClientPairInit),

    /// Server nonce contribution (dynamic pairing code flow)
    #[serde(rename = "server/pair-init")]
    ServerPairInit(ServerPairInit),

    /// Server CPace public share
    #[serde(rename = "server/pair-auth")]
    ServerPairAuth(ServerPairAuth),

    /// Client CPace public share
    #[serde(rename = "client/pair-auth")]
    ClientPairAuth(ClientPairAuth),

    /// Server MCF confirmation tag
    #[serde(rename = "server/pair-confirm")]
    ServerPairConfirm(ServerPairConfirm),

    /// Client MCF confirmation tag (plus sealed commitment opening)
    #[serde(rename = "client/pair-confirm")]
    ClientPairConfirm(ClientPairConfirm),

    /// Client delivers the new long-term PSK
    #[serde(rename = "client/pair-finalize")]
    ClientPairFinalize(ClientPairFinalize),

    /// Server acknowledges the persisted pairing record
    #[serde(rename = "server/pair-finalize")]
    ServerPairFinalize(ServerPairFinalize),

    /// Either side aborts a pairing attempt
    #[serde(rename = "pair/abort")]
    PairAbort(PairAbort),

    // === Connection lifecycle ===
    /// Paired server drops its own pairing record from the client
    #[serde(rename = "server/unpair")]
    ServerUnpair(ServerUnpair),

    /// Client goodbye message
    #[serde(rename = "client/goodbye")]
    ClientGoodbye(ClientGoodbye),
}

// =============================================================================
// Cleartext Handshake Messages
// =============================================================================

/// First message sent by the client after the WebSocket connection is
/// established (WebSocket text frame). Carries what the Noise handshake needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInit {
    /// Client's static public key (43-char base64url Curve25519, no padding)
    pub client_id: String,
    /// Core message format version (must be `1`, exact match)
    pub version: u32,
    /// Noise cipher suite the client picked for this connection
    pub suite: String,
}

/// Response to `client/init` (WebSocket text frame).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInit {
    /// Server's static public key (43-char base64url Curve25519, no padding)
    pub server_id: String,
    /// Core message format version (must be `1`, exact match)
    pub version: u32,
}

/// Carries one Noise handshake message. Sent as a WebSocket text frame during
/// the initial handshake, or as an encrypted JSON message during an in-band
/// re-handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseHandshake {
    /// base64url-encoded Noise handshake message bytes (no padding)
    pub data: String,
}

/// The inner payload of Noise message 1 (server → client): identifies the PSK
/// to mix in before processing message 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseMessage1Payload {
    /// 43-char base64url SHA-256 psk_id derived from the PSK
    pub psk_id: String,
    /// The category the server is using the referenced PSK as. A `psk_id` the
    /// client holds only under a different category is a lookup miss.
    pub psk_category: PskCategory,
}

/// PSK category declared in Noise message 1. The codes share one length, so
/// the encrypted payload's length is independent of the category.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PskCategory {
    /// Long-term PSK from a pairing record
    #[serde(rename = "lt")]
    LongTerm,
    /// The client's pairing PSK
    #[serde(rename = "pr")]
    Pairing,
    /// The published Sentinel PSK
    #[serde(rename = "sn")]
    Sentinel,
}

// =============================================================================
// Encrypted Handshake Messages
// =============================================================================

/// Client hello message, sent encrypted after `server/hello`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    /// Human-readable client name
    pub name: String,
    /// Device information (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_info: Option<DeviceInfo>,
    /// List of supported roles with versions (e.g., "player@v1", "controller@v1")
    pub supported_roles: Vec<String>,
    /// Player capabilities (if client supports player@v1 role)
    #[serde(rename = "player@v1_support", skip_serializing_if = "Option::is_none")]
    pub player_v1_support: Option<PlayerV1Support>,
    /// Source capabilities (if client supports source@v1 role)
    #[serde(rename = "source@v1_support", skip_serializing_if = "Option::is_none")]
    pub source_v1_support: Option<SourceV1Support>,
    /// Visualizer capabilities (if client supports visualizer@v1 role)
    #[serde(
        rename = "visualizer@v1_support",
        skip_serializing_if = "Option::is_none"
    )]
    pub visualizer_v1_support: Option<VisualizerV1Support>,
    /// Pairing methods this client currently offers, keyed by method
    pub supported_pair_methods: SupportedPairMethods,
    /// Whether this client currently admits unpaired access
    pub unpaired_access: UnpairedAccess,
}

/// Unpaired-access advertisement in `client/hello`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnpairedAccess {
    /// Whether unpaired access is currently enabled
    pub enabled: bool,
}

/// Pairing methods the client offers, keyed by method identifier. Every
/// client offers at least the Pairing PSK method; at most one pairing-code
/// method may be listed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupportedPairMethods {
    /// Pairing authenticated by the client's pairing PSK
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing_psk: Option<PairingPskDescriptor>,
    /// Fixed 8-digit pairing code (PAKE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_pairing_code: Option<StaticPairingCodeDescriptor>,
    /// Per-session pairing code bound to the Noise handshake (PAKE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_pairing_code: Option<DynamicPairingCodeDescriptor>,
}

/// Descriptor for the Pairing PSK method.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingPskDescriptor {
    /// Where the operator can find the pairing token (informational)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<PairingSecretLocation>>,
}

/// Descriptor for the static pairing code method.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StaticPairingCodeDescriptor {
    /// Where the operator can find the configured code (informational)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<PairingSecretLocation>>,
}

/// Descriptor for the dynamic pairing code method.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DynamicPairingCodeDescriptor {
    /// Channels through which the per-session code reaches the operator
    pub out_channels: Vec<PairingOutChannel>,
    /// Emission formats the client offers (non-empty)
    pub formats: Vec<PairingCodeFormat>,
    /// The server-supplied digit audio pack the client wants. Required when
    /// `out_channels` includes `speaker`, absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digit_audio: Option<DigitAudio>,
}

/// Digit audio pack request in a `dynamic_pairing_code` descriptor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DigitAudio {
    /// Codec identifier ('opus' | 'flac' | 'pcm')
    pub codec: String,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Bit depth; meaningful for `pcm` and `flac` only
    pub bit_depth: u8,
    /// Maximum total encoded size of the ten clips in bytes
    pub max_bytes: u32,
}

/// Pairing method identifier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingMethod {
    /// Pairing authenticated by the client's Pairing PSK
    PairingPsk,
    /// Per-session pairing code bound to the Noise handshake (PAKE)
    DynamicPairingCode,
    /// Fixed 8-digit pairing code (PAKE)
    StaticPairingCode,
}

/// Out-channel through which a dynamic pairing code is emitted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingOutChannel {
    /// The code is shown on a display
    Display,
    /// The code is spoken through a speaker
    Speaker,
}

/// Dynamic pairing code emission format.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingCodeFormat {
    /// 6-digit decimal code typed by the operator
    Digits,
    /// 24-byte code rendered as a QR code
    QrCode,
}

/// Where a static pairing secret can be found by the operator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairingSecretLocation {
    /// Printed on the device
    Device,
    /// On a leaflet in the box
    Leaflet,
    /// Set by the operator
    Operator,
}

/// An activity the server may declare on a connection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    /// Normal playback and control flows
    Playback,
    /// A pairing exchange
    Pairing,
}

/// Declares the server's current purpose on this connection. May be re-sent
/// at any time to change the activity set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerActivate {
    /// The set of currently-active purposes on this connection (may be empty)
    pub activities: Vec<Activity>,
    /// Versioned roles active for this client. Required on the first
    /// `server/activate`; persists when omitted afterwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_roles: Option<Vec<String>>,
    /// Parameters of the pairing attempt this activation admits (required
    /// when `activities` includes `pairing`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing: Option<ActivatePairing>,
}

/// Pairing parameters carried in `server/activate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivatePairing {
    /// Pairing method the server picked
    pub method: PairingMethod,
    /// Dynamic pairing code emission format (required for `dynamic_pairing_code`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<PairingCodeFormat>,
}

/// Device information (all fields optional per spec)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Product name (e.g., "Sendspin-RS Player")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_name: Option<String>,
    /// Manufacturer name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Software version string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software_version: Option<String>,
    /// MAC address of the network interface the connection is opened on, in lowercase colon-separated form (e.g., `aa:bb:cc:dd:ee:ff`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
}

/// Player@v1 capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerV1Support {
    /// List of supported audio formats in priority order (first is preferred)
    pub supported_formats: Vec<AudioFormatSpec>,
    /// Max size in bytes of compressed, not-yet-played audio messages in the buffer
    pub buffer_capacity: u32,
}

/// Source@v1 capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceV1Support {
    /// Optional feature hints
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<SourceFeatures>,
}

/// Source feature hints
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceFeatures {
    /// True if the source reports `signal` in `client/state`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_sense: Option<bool>,
}

/// Audio format specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioFormatSpec {
    /// Codec name (e.g., "pcm", "opus", "flac")
    pub codec: String,
    /// Number of audio channels
    pub channels: u8,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Bit depth per sample
    pub bit_depth: u8,
}

/// Artwork state object in `client/state`: the configuration the client
/// wants for each artwork channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtworkState {
    /// Per-channel configuration, positional from channel 0 (length 1-4). A
    /// channel index the array does not cover is `source: 'none'`.
    pub channels: Vec<ArtworkChannelConfig>,
}

impl ArtworkState {
    /// Validate the channel-count bound and per-channel field requirements.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.channels.is_empty() || self.channels.len() > 4 {
            return Err("artwork channels array must contain 1-4 entries");
        }
        for channel in &self.channels {
            channel.validate()?;
        }
        Ok(())
    }
}

/// Configuration for one artwork channel (in `client/state` and
/// `stream/start`). `format`, `width`, and `height` are required unless
/// `source` is `'none'`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtworkChannelConfig {
    /// Artwork source type
    pub source: ArtworkSource,
    /// Image format identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ImageFormat>,
    /// Width in pixels of the delivered image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Height in pixels of the delivered image
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl ArtworkChannelConfig {
    /// A disabled channel (`source: 'none'`).
    pub fn none() -> Self {
        Self {
            source: ArtworkSource::None,
            format: None,
            width: None,
            height: None,
        }
    }

    /// Validate the source-dependent field requirements.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.source != ArtworkSource::None
            && (self.format.is_none() || self.width.is_none() || self.height.is_none())
        {
            return Err("artwork channel with a source requires format, width, and height");
        }
        Ok(())
    }
}

/// Artwork source type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArtworkSource {
    /// Album artwork
    Album,
    /// Artist image
    Artist,
    /// No artwork (channel disabled)
    None,
}

/// Image format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// JPEG format
    Jpeg,
    /// PNG format
    Png,
}

/// Visualizer@v1 capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizerV1Support {
    /// Maximum total size of buffered visualizer messages in bytes.
    pub buffer_capacity: u32,
}

/// Visualizer state object in `client/state`: the requested data types,
/// frame-rate cap, and spectrum configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VisualizerState {
    /// Visualization data types requested by the client.
    pub types: Vec<VisualizerDataType>,
    /// Maximum periodic visualization frames per second.
    pub rate_max: u32,
    /// Spectrum configuration, required when `types` includes `spectrum`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<SpectrumConfig>,
}

impl VisualizerState {
    /// Validate the cross-field spectrum requirement from the Sendspin spec.
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_spectrum(&self.types, self.spectrum.as_ref())
    }
}

/// Visualization data type carried by a visualizer binary message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualizerDataType {
    /// Overall A-weighted loudness.
    Loudness,
    /// Musical beat event.
    Beat,
    /// Dominant frequency and amplitude.
    FPeak,
    /// FFT magnitudes mapped to display bins.
    Spectrum,
    /// Energy onset event.
    Peak,
}

/// Spectrum display-bin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpectrumConfig {
    /// Number of display bins.
    pub n_disp_bins: u32,
    /// Frequency-to-bin mapping.
    pub scale: SpectrumScale,
    /// Lowest frequency in Hz.
    pub f_min: u32,
    /// Highest frequency in Hz.
    pub f_max: u32,
}

fn validate_spectrum(
    types: &[VisualizerDataType],
    spectrum: Option<&SpectrumConfig>,
) -> Result<(), &'static str> {
    let has_spectrum = types.contains(&VisualizerDataType::Spectrum);
    match (has_spectrum, spectrum.is_some()) {
        (true, false) => Err("spectrum configuration is required for spectrum data"),
        (false, true) => Err("spectrum configuration requires spectrum data"),
        _ => Ok(()),
    }
}

/// Frequency-to-bin mapping for spectrum data.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpectrumScale {
    /// HTK mel-frequency spacing.
    Mel,
    /// Base-10 logarithmic spacing.
    Log,
    /// Linear frequency spacing.
    Lin,
}

/// Server hello message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerHello {
    /// Human-readable server name
    pub name: String,
    /// BCP 47 language tags in descending operator preference (non-empty when
    /// present) - a hint about the languages the operator understands
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<String>>,
}

// =============================================================================
// Time Synchronization
// =============================================================================

/// Client time sync message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientTime {
    /// Client transmission timestamp (raw monotonic microseconds)
    pub client_transmitted: i64,
}

/// Server time sync response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerTime {
    /// Original client transmission timestamp
    pub client_transmitted: i64,
    /// Server reception timestamp (server loop microseconds)
    pub server_received: i64,
    /// Server transmission timestamp (server loop microseconds)
    pub server_transmitted: i64,
}

// =============================================================================
// State Messages
// =============================================================================

/// Client state update message (wraps role-specific state).
///
/// Every message carries `available` and the full state of each role object
/// it includes; omitting a role object leaves that role's state unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientState {
    /// Whether the client is available to participate in Sendspin playback.
    /// `false` means the client's output is in use by an external system.
    pub available: bool,
    /// Player state (if player role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerState>,
    /// Source state (if source role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceState>,
    /// Artwork channel configuration (if artwork role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<ArtworkState>,
    /// Visualizer configuration (if visualizer role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visualizer: Option<VisualizerState>,
}

/// Source state object in `client/state`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceState {
    /// Line sensing / signal presence (only if `line_sense` is supported)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<SourceSignal>,
}

/// Signal presence reported by a line-sensing source.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceSignal {
    /// A signal is present on the input
    Present,
    /// No signal on the input
    Absent,
}

/// Player state object in `client/state`. Every included player object
/// carries the player's full state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerState {
    /// Current volume level (0-100). Must be included when `volume` is in
    /// `supported_commands`; a player may also report it read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Whether audio is muted. Must be included when `mute` is in
    /// `supported_commands`; a player may also report it read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// Output delay in milliseconds (0-5000) to compensate for external speaker/amplifier latency
    pub output_delay_ms: u16,
    /// Minimum startup lead time in milliseconds.
    pub required_lead_time_ms: u32,
    /// Requested minimum ongoing buffer duration in milliseconds.
    pub min_buffer_ms: u32,
    /// Commands the server may send; empty when the player accepts none.
    /// Advertises settability, not reportability.
    pub supported_commands: Vec<PlayerStateCommand>,
    /// The format the player currently prefers (one of its
    /// `supported_formats`). Absent means no overridden preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<AudioFormatSpec>,
}

impl PlayerState {
    /// Validate the fields whose requirements are conditional on the player's
    /// advertised capabilities.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.output_delay_ms > 5000 {
            return Err("output_delay_ms must be in the range 0-5000");
        }
        if self
            .supported_commands
            .contains(&PlayerStateCommand::Volume)
            && self.volume.is_none()
        {
            return Err("volume is required when supported_commands includes volume");
        }
        if self.supported_commands.contains(&PlayerStateCommand::Mute) && self.muted.is_none() {
            return Err("muted is required when supported_commands includes mute");
        }
        Ok(())
    }
}

/// Commands that can appear in PlayerState.supported_commands
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerStateCommand {
    /// The server may set the player's volume
    Volume,
    /// The server may set the player's mute state
    Mute,
    /// The server may set the player's output delay
    SetOutputDelay,
}

/// Server state update message (metadata and controller info).
///
/// Every message carries the full state of each role object it includes;
/// omitting a role object leaves that role's state unchanged (and any
/// pending scheduled update in place). Each role object distinguishes
/// *absent* (no change, outer `None`) from *null* (clear all of that role's
/// state immediately, discarding any pending scheduled update: `Some(None)`).
///
/// For `metadata` and `color`, a future `timestamp` is a **scheduled
/// update**: keep the current state plus at most one pending update, apply
/// the pending one when its (time-filter-translated) timestamp is reached,
/// and let a past-or-present-timestamped message apply immediately and
/// discard any pending update.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerState {
    /// Metadata state (track info, progress, etc.)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "double_option"
    )]
    pub metadata: Option<Option<MetadataState>>,
    /// Controller state (supported commands, volume, etc.)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "double_option"
    )]
    pub controller: Option<Option<ControllerState>>,
    /// Color state (colors derived from the current audio)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "double_option"
    )]
    pub color: Option<Option<ColorState>>,
}

/// Deserialize a present-but-possibly-null field into `Some(Option<T>)`,
/// so `#[serde(default)]` yields `None` only when the key is absent.
fn double_option<'de, T, D>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// An RGB color as `[R, G, B]` with components 0-255.
pub type Rgb = [u8; 3];

/// Color state from server (color role).
///
/// Colors may be extracted from album artwork, provided by the music source,
/// or manually programmed by the server. All color fields are optional; the
/// server sends only the palette entries it has.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorState {
    /// Server clock time in microseconds at which these colors take effect.
    /// A future timestamp schedules the update (see [`ServerState`]).
    pub timestamp: i64,
    /// Background color suitable for dark mode. The server ensures a minimum
    /// WCAG contrast ratio of 4.5:1 with white text and with `on_dark`
    /// (if also present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_dark: Option<Rgb>,
    /// Background color suitable for light mode. The server ensures a minimum
    /// WCAG contrast ratio of 4.5:1 with black text and with `on_light`
    /// (if also present).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_light: Option<Rgb>,
    /// The dominant color. Not adjusted for contrast.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<Rgb>,
    /// A secondary or complementary color. Not adjusted for contrast.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<Rgb>,
    /// A light color suitable for use on dark backgrounds. The server ensures
    /// a minimum WCAG contrast ratio of 4.5:1 with `background_dark` (if also
    /// present) and with black text, so it can also serve as an alternative
    /// light background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_dark: Option<Rgb>,
    /// A dark color suitable for use on light backgrounds. The server ensures
    /// a minimum WCAG contrast ratio of 4.5:1 with `background_light` (if also
    /// present) and with white text, so it can also serve as an alternative
    /// dark background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_light: Option<Rgb>,
}

/// Metadata state from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataState {
    /// Server clock time in microseconds at which this metadata takes effect,
    /// and the point progress extrapolation runs from. A future timestamp
    /// schedules the update (see [`ServerState`]).
    pub timestamp: i64,
    /// Track title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Artist name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    /// Album artist
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    /// Album name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// Artwork URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    /// Release year
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    /// Track number (1-indexed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<u32>,
    /// Current track progress
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<TrackProgress>,
}

/// Track progress information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackProgress {
    /// Current position in milliseconds
    pub track_progress: i64,
    /// Total duration in milliseconds (0 for unknown/live streams)
    pub track_duration: i64,
    /// Playback speed multiplier * 1000 (1000 = normal, 0 = paused)
    pub playback_speed: i32,
}

/// Repeat mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    /// No repeat
    Off,
    /// Repeat current track
    One,
    /// Repeat all tracks
    All,
}

/// Controller state from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerState {
    /// List of supported commands
    pub supported_commands: Vec<String>,
    /// Current volume level (0-100)
    pub volume: u8,
    /// Whether audio is muted
    pub muted: bool,
    /// Repeat mode
    pub repeat: RepeatMode,
    /// Shuffle state
    pub shuffle: bool,
    /// Maximum absolute position in milliseconds a 'seek' may target (e.g.,
    /// the end of the current track). Present whenever 'seek' is in
    /// `supported_commands`; absent when the seekable range is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seek_max_ms: Option<u64>,
}

// =============================================================================
// Command Messages
// =============================================================================

/// Server command message (wraps role-specific commands)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCommand {
    /// Player command (if targeting player role)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerCommand>,
    /// Source command (if targeting source role)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceCommand>,
}

/// Source-specific command from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCommand {
    /// Command to execute
    pub command: SourceCommandType,
}

/// Source command type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceCommandType {
    /// Begin streaming captured audio to the server
    Start,
    /// Stop streaming captured audio
    Stop,
    /// Unknown command (forward compatibility)
    #[serde(other)]
    Unknown,
}

/// Player-specific command from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerCommand {
    /// Command to execute
    pub command: PlayerCommandType,
    /// Optional volume level (0-100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Optional mute state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// Optional output delay in milliseconds (0-5000)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_delay_ms: Option<u16>,
}

/// Player command type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerCommandType {
    /// Set volume level
    Volume,
    /// Set mute state
    Mute,
    /// Set output delay
    SetOutputDelay,
    /// Unknown command (forward compatibility)
    #[serde(other)]
    Unknown,
}

/// Client command message (controller commands to server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCommand {
    /// Controller command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<ControllerCommand>,
}

/// Controller command from client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControllerCommand {
    /// Command to execute
    pub command: ControllerCommandType,
    /// Optional volume level (0-100) for volume command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Optional mute state for mute command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    /// Absolute playback position in milliseconds, range 0 to
    /// [`ControllerState::seek_max_ms`]. Only set for the `seek` command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ms: Option<u64>,
    /// Signed offset in milliseconds from the current position (positive
    /// forward, negative backward). Only set for the `seek_relative` command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_ms: Option<i64>,
}

/// Controller command type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControllerCommandType {
    /// Resume playback
    Play,
    /// Pause playback
    Pause,
    /// Stop playback and reset position
    Stop,
    /// Skip to next track
    Next,
    /// Skip to previous track
    Previous,
    /// Set group volume
    Volume,
    /// Set group mute state
    Mute,
    /// Disable repeat
    RepeatOff,
    /// Repeat current track
    RepeatOne,
    /// Repeat all tracks
    RepeatAll,
    /// Randomize playback order
    Shuffle,
    /// Restore original playback order
    Unshuffle,
    /// Switch to next group
    Switch,
    /// Seek to an absolute position (requires `position_ms`)
    Seek,
    /// Seek by a signed offset from the current position (requires `offset_ms`)
    SeekRelative,
}

// =============================================================================
// Stream Control Messages
// =============================================================================

/// Stream start message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStart {
    /// Timestamp that the server transmitted this message in microseconds
    pub server_transmitted: i64,
    /// Player stream configuration (optional - only if player role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<StreamPlayerConfig>,
    /// Artwork stream configuration (optional - only if artwork role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<StreamArtworkConfig>,
    /// Visualizer stream configuration (optional - only if visualizer role active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visualizer: Option<StreamVisualizerConfig>,
}

/// Stream player configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamPlayerConfig {
    /// Audio codec name
    pub codec: String,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of audio channels
    pub channels: u8,
    /// Bit depth per sample
    pub bit_depth: u8,
    /// Optional codec-specific header (base64 encoded)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_header: Option<String>,
}

/// Stream artwork configuration: per-channel configuration, positional from
/// channel 0 and never longer than 4. A channel the array does not cover, or
/// whose `source` is `'none'`, is not streamed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamArtworkConfig {
    /// Configuration for each artwork channel, array index is the channel number
    pub channels: Vec<ArtworkChannelConfig>,
}

/// Stream visualizer configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamVisualizerConfig {
    /// Visualization data types the server will stream.
    pub types: Vec<VisualizerDataType>,
    /// Maximum periodic visualization frames per second.
    pub rate_max: u32,
    /// Whether the beat tracker identifies bar starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks_downbeats: Option<bool>,
    /// Spectrum configuration, present when `types` includes `spectrum`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<SpectrumConfig>,
}

impl StreamVisualizerConfig {
    /// Validate the cross-field spectrum requirement from the Sendspin spec.
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_spectrum(&self.types, self.spectrum.as_ref())
    }
}

/// Stream end message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEnd {
    /// Roles for which streaming has ended (optional, all if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}

/// Stream clear message (clear buffers)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamClear {
    /// Timestamp that the server transmitted this message in microseconds
    pub server_transmitted: i64,
    /// Roles for which buffers should be cleared (optional, all if not specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}

// =============================================================================
// Source Stream Messages (client → server)
// =============================================================================

/// Announces the source's active input stream format (`client-stream/start`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientStreamStart {
    /// The input stream format
    pub source: SourceStreamConfig,
}

/// Source input stream format announcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceStreamConfig {
    /// Audio codec name ('opus' | 'flac' | 'pcm')
    pub codec: String,
    /// Number of audio channels
    pub channels: u8,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Bit depth per sample (ignored for opus)
    pub bit_depth: u8,
    /// Optional codec-specific header (standard Base64, padded), e.g. FLAC
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_header: Option<String>,
}

/// Ends the source's current input stream (`client-stream/end`). No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientStreamEnd {}

// =============================================================================
// Group Messages
// =============================================================================

/// Group update notification. Every message carries the full group state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupUpdate {
    /// Current playback state of the group
    pub playback_state: PlaybackState,
    /// Group identifier
    pub group_id: String,
    /// Human-readable group name
    pub group_name: String,
}

/// Group playback state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    /// Audio is playing
    Playing,
    /// Playback is stopped
    Stopped,
}

// =============================================================================
// Connection Lifecycle
// =============================================================================

/// Client goodbye message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientGoodbye {
    /// Reason for disconnection
    pub reason: GoodbyeReason,
}

/// Goodbye reason
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoodbyeReason {
    /// Switching to another server
    AnotherServer,
    /// Client is shutting down
    Shutdown,
    /// Client is restarting
    Restart,
    /// User requested disconnect
    UserRequest,
    /// The client is no longer authorized for the connection
    Unauthorized,
    /// The client refused an unpaired-access connection
    PairingRequired,
    /// A higher-or-equal-priority connection is already active
    ConcurrentAttempt,
    /// The client processed `server/unpair` from this server
    Unpaired,
}

/// Paired server drops its own pairing record from the client. No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerUnpair {}

// =============================================================================
// Pairing Messages
// =============================================================================

/// Reports that the selected attempt is gesture-gated and no pairing window is open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPairPending {
    /// Number of pairing `server/activate` messages received since the last Noise handshake
    pub pairing_index: u32,
}

/// Starts a code-based pairing attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPairInit {
    /// Number of pairing `server/activate` messages received since the last Noise handshake
    pub pairing_index: u32,
    /// Commitment to nonce_B (dynamic pairing code flow only), 43-char base64url
    #[serde(rename = "commit_B", skip_serializing_if = "Option::is_none")]
    pub commit_b: Option<String>,
}

/// Server's nonce contribution in the dynamic pairing code flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerPairInit {
    /// 32 CSPRNG bytes, base64url-encoded (43 chars)
    #[serde(rename = "nonce_A")]
    pub nonce_a: String,
}

/// Server's CPace public share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerPairAuth {
    /// Server's CPace public share `Ya` (43-char base64url)
    pub pake_msg_1: String,
}

/// Client's CPace public share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPairAuth {
    /// Client's CPace public share `Yb` (43-char base64url)
    pub pake_msg_2: String,
}

/// Server's MCF confirmation tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerPairConfirm {
    /// Server's MCF tag `Ta` (86-char base64url)
    pub server_kc: String,
}

/// Client's MCF confirmation tag plus the sealed commitment opening.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPairConfirm {
    /// Client's MCF tag `Tb` (86-char base64url)
    pub client_kc: String,
    /// Sealed opening of `commit_B` (dynamic pairing code flow only), 64-char base64url
    #[serde(rename = "wrapped_nonce_B", skip_serializing_if = "Option::is_none")]
    pub wrapped_nonce_b: Option<String>,
}

/// Delivers the long-term PSK for this (client, server) pair.
/// Exactly one of the two fields is present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientPairFinalize {
    /// The PSK, sent directly (Pairing PSK flow only), 43-char base64url
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_term_psk: Option<String>,
    /// The PSK, wrapped under the CPace output (code-based flows only), 64-char base64url
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapped_psk: Option<String>,
}

/// Acknowledges the server has persisted the pairing record. No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerPairFinalize {}

/// Aborts a pairing attempt, started or not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairAbort {
    /// Why the attempt was aborted
    pub reason: PairAbortReason,
}

/// Reason carried in `pair/abort`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PairAbortReason {
    /// The pairing attempt did not complete within the attempt timeout
    AttemptTimeout,
    /// Another pairing attempt is already in progress with this client
    ConcurrentAttempt,
    /// The activity set / pairing method / format combination is not permitted or offered
    MethodNotSupported,
    /// PAKE key-confirmation failed
    PairingCodeMismatch,
    /// Operator aborted the pairing through a local UI
    UserCancelled,
}
