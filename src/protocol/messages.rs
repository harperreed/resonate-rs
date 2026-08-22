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

    /// Client request for specific stream format
    #[serde(rename = "stream/request-format")]
    StreamRequestFormat(StreamRequestFormat),

    // === Source (client → server) stream messages ===
    /// Source input stream start (format announcement)
    #[serde(rename = "client_stream/start")]
    ClientStreamStart(ClientStreamStart),

    /// Source input stream end
    #[serde(rename = "client_stream/end")]
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

    // === Management messages ===
    /// List the client's pairing records
    #[serde(rename = "management/list-records")]
    ManagementListRecords(ManagementListRecords),

    /// Add a pairing record directly
    #[serde(rename = "management/add-record")]
    ManagementAddRecord(ManagementAddRecord),

    /// Remove a pairing record
    #[serde(rename = "management/remove-record")]
    ManagementRemoveRecord(ManagementRemoveRecord),

    /// Read the client's pairing configuration
    #[serde(rename = "management/get-pairing-config")]
    ManagementGetPairingConfig(ManagementGetPairingConfig),

    /// Modify the client's pairing configuration
    #[serde(rename = "management/set-pairing-config")]
    ManagementSetPairingConfig(ManagementSetPairingConfig),

    /// Open a pairing window in place of the operator gesture
    #[serde(rename = "management/open-pairing-window")]
    ManagementOpenPairingWindow(ManagementOpenPairingWindow),

    /// Client response to a `management/*` request
    #[serde(rename = "management/result")]
    ManagementResult(ManagementResult),

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
}

// =============================================================================
// Encrypted Handshake Messages
// =============================================================================

/// Trust level the client extends to the server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    /// A pairing record exists for this server
    User,
    /// No pairing record: pairing handshakes and unpaired access
    None,
}

/// Client hello message, sent encrypted after `server/hello`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHello {
    /// Human-readable client name
    pub name: String,
    /// Device information (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_info: Option<DeviceInfo>,
    /// Trust level the client extends to this server
    pub trust_level: TrustLevel,
    /// List of supported roles with versions (e.g., "player@v1", "controller@v1")
    pub supported_roles: Vec<String>,
    /// Player capabilities (if client supports player@v1 role)
    #[serde(rename = "player@v1_support", skip_serializing_if = "Option::is_none")]
    pub player_v1_support: Option<PlayerV1Support>,
    /// Source capabilities (if client supports source@v1 role)
    #[serde(rename = "source@v1_support", skip_serializing_if = "Option::is_none")]
    pub source_v1_support: Option<SourceV1Support>,
    /// Artwork capabilities (if client supports artwork@v1 role)
    #[serde(rename = "artwork@v1_support", skip_serializing_if = "Option::is_none")]
    pub artwork_v1_support: Option<ArtworkV1Support>,
    /// Visualizer capabilities (if client supports visualizer@v1 role)
    #[serde(
        rename = "visualizer@v1_support",
        skip_serializing_if = "Option::is_none"
    )]
    pub visualizer_v1_support: Option<VisualizerV1Support>,
    /// Pairing methods this client currently offers
    pub supported_pair_methods: Vec<PairMethodDescriptor>,
    /// Whether this client currently admits unpaired access
    pub unpaired_access: UnpairedAccess,
}

/// Unpaired-access advertisement in `client/hello`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnpairedAccess {
    /// Whether unpaired access is currently enabled
    pub enabled: bool,
}

/// A pairing method the client offers, with UX hints for the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairMethodDescriptor {
    /// The pairing method identifier
    pub method: PairingMethod,
    /// Out-channels conveying the dynamic pairing code (informational)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_channels: Option<Vec<PairingOutChannel>>,
    /// Emission formats offered (required on `dynamic_pairing_code`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formats: Option<Vec<PairingCodeFormat>>,
    /// Where the operator can find the configured secret (informational)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<PairingSecretLocation>>,
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
    /// Management operations
    Management,
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
    /// BCP 47 language tags in descending operator preference, for spoken emission
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<String>>,
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
    /// List of supported playback commands (subset of 'volume', 'mute')
    pub supported_commands: Vec<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Artwork@v1 capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkV1Support {
    /// Supported artwork channels (1-4 channels, array index is channel number)
    pub channels: Vec<ArtworkChannel>,
}

/// Artwork channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkChannel {
    /// Artwork source type
    pub source: ArtworkSource,
    /// Image format
    pub format: ImageFormat,
    /// Max width in pixels
    pub media_width: u32,
    /// Max height in pixels
    pub media_height: u32,
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
    /// BMP format
    Bmp,
}

/// Visualizer@v1 capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizerV1Support {
    /// Visualization data types requested by the client.
    pub types: Vec<VisualizerDataType>,
    /// Maximum total size of buffered visualizer messages in bytes.
    pub buffer_capacity: u32,
    /// Maximum periodic visualization frames per second.
    pub rate_max: u32,
    /// Spectrum configuration, required when `types` includes `spectrum`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<SpectrumConfig>,
}

impl VisualizerV1Support {
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

/// Client state update message (wraps role-specific state)
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

/// Player state
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerState {
    /// Current volume level (0-100)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// Whether audio is muted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// Output delay in milliseconds (0-5000) to compensate for external speaker/amplifier latency
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_delay_ms: Option<u16>,
    /// Minimum startup lead time in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub required_lead_time_ms: Option<u32>,
    /// Requested minimum ongoing buffer duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub min_buffer_ms: Option<u32>,
    /// Supported player state commands
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supported_commands: Option<Vec<PlayerStateCommand>>,
}

/// Commands that can appear in PlayerState.supported_commands
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlayerStateCommand {
    /// Client supports set_output_delay command
    SetOutputDelay,
}

/// Server state update message (metadata and controller info).
///
/// Each role object distinguishes *absent* (no change, outer `None`) from
/// *null* (clear all of that role's state, `Some(None)`).
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
    /// Server clock time in microseconds for when these colors are valid
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
    /// Server timestamp for progress calculation (microseconds)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<RepeatMode>,
    /// Shuffle state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shuffle: Option<bool>,
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

/// Stream artwork configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamArtworkConfig {
    /// Configuration for each active artwork channel, array index is the channel number
    pub channels: Vec<StreamArtworkChannelConfig>,
}

/// Configuration for a single artwork channel in stream/start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamArtworkChannelConfig {
    /// Artwork source type
    pub source: ArtworkSource,
    /// Format of the encoded image
    pub format: ImageFormat,
    /// Width in pixels of the encoded image
    pub width: u32,
    /// Height in pixels of the encoded image
    pub height: u32,
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
    /// Timestamp that the server transmitted this message in microseconds
    pub server_transmitted: i64,
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

/// Announces the source's active input stream format (`client_stream/start`).
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

/// Ends the source's current input stream (`client_stream/end`). No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientStreamEnd {}

/// Stream format request from client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRequestFormat {
    /// Requested player format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerFormatRequest>,
    /// Requested artwork format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<ArtworkFormatRequest>,
    /// Requested visualizer format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visualizer: Option<VisualizerFormatRequest>,
}

/// Requested visualizer stream format. Omitted fields keep their current value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VisualizerFormatRequest {
    /// New visualization data types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<VisualizerDataType>>,
    /// New periodic visualization frames-per-second cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_max: Option<u32>,
    /// New spectrum configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<SpectrumConfig>,
}

impl VisualizerFormatRequest {
    /// Validate that this request changes at least one field.
    ///
    /// This is a partial update: omitted fields retain their current value, so
    /// a request may omit `spectrum` even when its new `types` includes it.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.types.is_none() && self.rate_max.is_none() && self.spectrum.is_none() {
            return Err("visualizer format request must specify at least one field");
        }
        if let (Some(types), Some(spectrum)) = (self.types.as_ref(), self.spectrum.as_ref()) {
            validate_spectrum(types, Some(spectrum))
        } else {
            Ok(())
        }
    }
}

/// Player format request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerFormatRequest {
    /// Preferred codec
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Preferred channel count
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
    /// Preferred sample rate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// Preferred bit depth
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u8>,
}

/// Artwork format request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtworkFormatRequest {
    /// Artwork channel to request
    pub channel: u8,
    /// Preferred image source
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ArtworkSource>,
    /// Preferred image format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<ImageFormat>,
    /// Display width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_width: Option<u32>,
    /// Display height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_height: Option<u32>,
}

// =============================================================================
// Group Messages
// =============================================================================

/// Group update notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupUpdate {
    /// Current playback state of the group
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_state: Option<PlaybackState>,
    /// Group identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Human-readable group name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
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

// =============================================================================
// Management Messages
// =============================================================================

/// List the client's pairing records. No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManagementListRecords {}

/// Add a pairing record directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagementAddRecord {
    /// 43-char base64url 32-byte Sendspin PSK (no padding)
    pub psk: String,
    /// Present for stored-pubkey records, absent for shared-PSK records
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

/// Remove a pairing record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagementRemoveRecord {
    /// The record's psk_id
    pub psk_id: String,
}

/// Read the client's pairing configuration. No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManagementGetPairingConfig {}

/// Modify the client's pairing configuration (applied as a patch).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManagementSetPairingConfig {
    /// Pairing PSK method settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing_psk: Option<PairingPskConfigPatch>,
    /// Static pairing code method settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_pairing_code: Option<StaticPairingCodeConfigPatch>,
    /// Dynamic pairing code method settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_pairing_code: Option<DynamicPairingCodeConfigPatch>,
    /// Record-mode setting (storage-exhaustion fallback record)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_mode: Option<RecordMode>,
    /// Unpaired-access setting
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unpaired_access: Option<UnpairedAccessPatch>,
}

/// Patch for the Pairing PSK method config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairingPskConfigPatch {
    /// Enable or disable the method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Replace the configured Pairing PSK (43-char base64url)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psk: Option<String>,
}

/// Patch for the static pairing code method config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaticPairingCodeConfigPatch {
    /// Enable or disable the method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Replace the configured static pairing code (8 decimal digits)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Patch for the dynamic pairing code method config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DynamicPairingCodeConfigPatch {
    /// Enable or disable the method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Patch for the unpaired-access toggle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnpairedAccessPatch {
    /// Enable or disable unpaired access
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Record-mode setting: the shared-PSK record used as the storage-exhaustion fallback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordMode {
    /// psk_id of the fallback shared-PSK record
    pub psk_id: String,
}

/// Open a pairing window in place of the operator gesture. No payload fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManagementOpenPairingWindow {}

/// Response to a `management/*` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagementResult {
    /// Result code
    pub result: ManagementResultCode,
    /// Operation-specific response payload (present only when defined and `result` is `ok`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Storage accounting, from clients that track bounded storage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageAccounting>,
}

/// Management result code.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagementResultCode {
    /// Operation completed and any state change has been persisted
    Ok,
    /// The request was issued outside a valid management session
    PermissionDenied,
    /// The request conflicts with an existing entry on the client
    AlreadyExists,
    /// Malformed payload, out-of-range value, missing field, or referential violation
    Invalid,
    /// The request targets an identifier that does not exist on the client
    NotFound,
    /// The client cannot persist the change due to full storage
    StorageExhausted,
}

/// Storage accounting reported in `management/result`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageAccounting {
    /// Currently free space (always present)
    pub free: u64,
    /// Total pool size (on list-records / get-pairing-config results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<u64>,
    /// Cost of a new stored-pubkey record
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_individual: Option<u64>,
    /// Cost of a new shared-PSK record
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_shared: Option<u64>,
}

/// A pairing record entry in `management/list-records` result data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordEntry {
    /// The record's psk_id
    pub psk_id: String,
    /// Present for stored-pubkey records, absent for shared-PSK records
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    /// True once a server has authenticated a session with this record's PSK
    pub used: bool,
}
