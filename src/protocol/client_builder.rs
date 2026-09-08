// ABOUTME: Builder exposed for public usage of the library
// ABOUTME: Assembles identity, PSK candidates, and hello capabilities into a SessionConfig

use crate::error::Error;
use crate::protocol::client::SessionConfig;
use crate::protocol::crypto::{CipherSuite, ClientCredentials, PskCandidate};
use crate::protocol::listener::ProtocolListener;
use crate::protocol::messages::{
    ArtworkState, AudioFormatSpec, ClientState, DeviceInfo, PlayerState, PlayerStateCommand,
    PlayerV1Support, SourceV1Support, SupportedPairMethods, VisualizerState, VisualizerV1Support,
};
use crate::protocol::pairing::{candidates_from, MemoryPairingStore, PairingStore};
use crate::protocol::session::HelloTemplate;
use crate::sync::raw_clock::{Clock, DefaultClock};
use crate::ProtocolClient;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::WebSocketStream;
use typed_builder::TypedBuilder;

/// Intermediate builder struct before finalization
#[derive(Clone)]
pub(crate) struct ProtocolClientBuilderRaw {
    credentials: ClientCredentials,
    suite: CipherSuite,
    psk_records: Vec<PskCandidate>,
    pairing_store: Option<Arc<dyn PairingStore>>,
    unpaired_access: bool,
    name: String,
    product_name: Option<String>,
    manufacturer: Option<String>,
    software_version: Option<String>,
    mac_address: Option<String>,
    player_v1_support: Option<PlayerV1Support>,
    source_v1_support: Option<SourceV1Support>,
    artwork_state: Option<ArtworkState>,
    visualizer_v1_support: Option<VisualizerV1Support>,
    visualizer_state: Option<VisualizerState>,
    initial_available: bool,
    max_encoded_artwork_transfer_bytes: usize,
    initial_player_state: Option<PlayerState>,
    metadata: bool,
    controller: bool,
    color: bool,
}

impl From<ProtocolClientBuilderRaw> for ProtocolClientBuilder {
    fn from(raw: ProtocolClientBuilderRaw) -> Self {
        // Build supported_roles based on which supports are configured
        let mut supported_roles = Vec::new();
        let has_explicit_role = raw.player_v1_support.is_some()
            || raw.source_v1_support.is_some()
            || raw.artwork_state.is_some()
            || raw.visualizer_v1_support.is_some()
            || raw.metadata
            || raw.controller
            || raw.color;

        // Default to player@v1 if no roles were explicitly configured
        let player_v1_support = if has_explicit_role {
            raw.player_v1_support
        } else {
            Some(PlayerV1Support {
                supported_formats: vec![
                    AudioFormatSpec {
                        codec: "opus".to_string(),
                        channels: 2,
                        sample_rate: 48000,
                        bit_depth: 16,
                    },
                    AudioFormatSpec {
                        codec: "pcm".to_string(),
                        channels: 2,
                        sample_rate: 48000,
                        bit_depth: 24,
                    },
                    AudioFormatSpec {
                        codec: "pcm".to_string(),
                        channels: 2,
                        sample_rate: 48000,
                        bit_depth: 16,
                    },
                ],
                buffer_capacity: 50 * 1024 * 1024,
            })
        };

        if player_v1_support.is_some() {
            supported_roles.push("player@v1".to_string());
        }
        if raw.source_v1_support.is_some() {
            supported_roles.push("source@v1".to_string());
        }
        if raw.artwork_state.is_some() {
            supported_roles.push("artwork@v1".to_string());
        }
        if raw.visualizer_v1_support.is_some() {
            supported_roles.push("visualizer@v1".to_string());
        }
        if raw.metadata {
            supported_roles.push("metadata@v1".to_string());
        }
        if raw.controller {
            supported_roles.push("controller@v1".to_string());
        }
        if raw.color {
            supported_roles.push("color@v1".to_string());
        }

        ProtocolClientBuilder {
            credentials: raw.credentials,
            suite: raw.suite,
            psk_records: raw.psk_records,
            // Resolve the default once, when this builder is finalized. The
            // resulting Arc is cloned by listeners for every accepted peer,
            // preserving pairing records across reconnects.
            pairing_store: Some(
                raw.pairing_store
                    .unwrap_or_else(|| Arc::new(MemoryPairingStore::new())),
            ),
            unpaired_access: raw.unpaired_access,
            name: raw.name,
            product_name: raw.product_name,
            manufacturer: raw.manufacturer,
            software_version: raw.software_version,
            mac_address: raw.mac_address,
            supported_roles,
            player_v1_support,
            clock: Arc::new(DefaultClock::new()),
            source_v1_support: raw.source_v1_support,
            artwork_state: raw.artwork_state,
            visualizer_v1_support: raw.visualizer_v1_support,
            visualizer_state: raw.visualizer_state,
            initial_available: raw.initial_available,
            max_encoded_artwork_transfer_bytes: raw.max_encoded_artwork_transfer_bytes,
            initial_player_state: raw.initial_player_state,
        }
    }
}

#[derive(TypedBuilder, Clone)]
#[builder(build_method(into = ProtocolClientBuilder))]
/// Builder Class for ProtocolClient
pub struct ProtocolClientBuilderFields {
    /// Human-readable client name
    name: String,
    /// The client's stable identity and mandatory Pairing PSK. Generate this
    /// once, persist [`ClientCredentials::to_bytes`] in application-owned
    /// secure storage, and reuse it across restarts. The same value can produce
    /// the pairing token before the first connection.
    credentials: ClientCredentials,
    /// The Noise cipher suite announced in `client/init`.
    #[builder(default = CipherSuite::ChaChaPoly)]
    suite: CipherSuite,
    /// Extra long-term pairing-record PSK candidates (each bound to a
    /// `server_id`), in addition to those loaded from the pairing store.
    #[builder(default = Vec::new())]
    psk_records: Vec<PskCandidate>,
    /// Persistence for pairing records: existing records become handshake
    /// candidates and newly paired records are written here. When omitted,
    /// the builder creates one shared in-memory store; listener clones share
    /// it across accepted connections, but it does not survive process
    /// restarts. Supply an application-owned store for durable persistence.
    #[builder(default = None, setter(transform = |x: Arc<dyn PairingStore>| Some(x)))]
    pairing_store: Option<Arc<dyn PairingStore>>,
    /// Whether this client admits unpaired (Sentinel-PSK) playback sessions.
    /// Defaults to `true` for frictionless setups; products handling
    /// sensitive inputs (e.g. a microphone source) should disable it and pair.
    #[builder(default = true)]
    unpaired_access: bool,
    #[builder(default = None)]
    product_name: Option<String>,
    #[builder(default = None)]
    manufacturer: Option<String>,
    #[builder(default = None)]
    software_version: Option<String>,
    #[builder(default = None)]
    mac_address: Option<String>,
    #[builder(default = None, setter(transform = |x: PlayerV1Support| Some(x)))]
    player_v1_support: Option<PlayerV1Support>,
    #[builder(default = None, setter(transform = |x: SourceV1Support| Some(x)))]
    source_v1_support: Option<SourceV1Support>,
    /// Declares the `artwork@v1` role: the per-channel configuration this
    /// client wants, sent in the initial `client/state` artwork object.
    #[builder(default = None, setter(transform = |x: ArtworkState| Some(x)))]
    artwork_state: Option<ArtworkState>,
    /// Declares the `visualizer@v1` role capability (`buffer_capacity`).
    /// Pair this with [`Self::visualizer_state`] to provide the initial
    /// `client/state` visualizer object; both halves are validated together
    /// before outbound connection establishment. For
    /// [`ProtocolClientBuilder::listen`], the TCP listener is bound first and
    /// each accepted WebSocket is validated when passed to
    /// [`ProtocolClientBuilder::accept`].
    #[builder(default = None, setter(transform = |x: VisualizerV1Support| Some(x)))]
    visualizer_v1_support: Option<VisualizerV1Support>,
    /// The requested visualizer data types, frame-rate cap, and spectrum
    /// configuration, sent in the initial `client/state` visualizer object.
    /// Pair this with [`Self::visualizer_v1_support`].
    #[builder(default = None, setter(transform = |x: VisualizerState| Some(x)))]
    visualizer_state: Option<VisualizerState>,
    /// Initial top-level availability sent in the first `client/state`.
    /// `false` when another source owns the output at connect time.
    #[builder(default = true)]
    initial_available: bool,
    /// Maximum encoded JPEG/PNG bytes accepted in one artwork transfer. This
    /// is the aggregate artwork announce `total_size`, not pixel dimensions,
    /// decoded image memory, one message, or transport framing overhead.
    #[builder(default = crate::protocol::binary::DEFAULT_MAX_ENCODED_ARTWORK_TRANSFER_BYTES)]
    max_encoded_artwork_transfer_bytes: usize,
    #[builder(default = None, setter(transform = |x: PlayerState| Some(x)))]
    initial_player_state: Option<PlayerState>,
    #[builder(default = false, setter(transform = || true))]
    metadata: bool,
    #[builder(default = false, setter(transform = || true))]
    controller: bool,
    #[builder(default = false, setter(transform = || true))]
    color: bool,
}

impl From<ProtocolClientBuilderFields> for ProtocolClientBuilder {
    fn from(fields: ProtocolClientBuilderFields) -> Self {
        let raw = ProtocolClientBuilderRaw {
            credentials: fields.credentials,
            suite: fields.suite,
            psk_records: fields.psk_records,
            pairing_store: fields.pairing_store,
            unpaired_access: fields.unpaired_access,
            name: fields.name,
            product_name: fields.product_name,
            manufacturer: fields.manufacturer,
            software_version: fields.software_version,
            mac_address: fields.mac_address,
            player_v1_support: fields.player_v1_support,
            source_v1_support: fields.source_v1_support,
            artwork_state: fields.artwork_state,
            visualizer_v1_support: fields.visualizer_v1_support,
            visualizer_state: fields.visualizer_state,
            initial_available: fields.initial_available,
            max_encoded_artwork_transfer_bytes: fields.max_encoded_artwork_transfer_bytes,
            initial_player_state: fields.initial_player_state,
            metadata: fields.metadata,
            controller: fields.controller,
            color: fields.color,
        };
        raw.into()
    }
}

/// Builder Class for ProtocolClient
#[derive(Clone)]
pub struct ProtocolClientBuilder {
    credentials: ClientCredentials,
    suite: CipherSuite,
    psk_records: Vec<PskCandidate>,
    pairing_store: Option<Arc<dyn PairingStore>>,
    unpaired_access: bool,
    name: String,
    product_name: Option<String>,
    manufacturer: Option<String>,
    software_version: Option<String>,
    mac_address: Option<String>,
    supported_roles: Vec<String>,
    player_v1_support: Option<PlayerV1Support>,
    source_v1_support: Option<SourceV1Support>,
    artwork_state: Option<ArtworkState>,
    visualizer_v1_support: Option<VisualizerV1Support>,
    visualizer_state: Option<VisualizerState>,
    initial_available: bool,
    max_encoded_artwork_transfer_bytes: usize,
    initial_player_state: Option<PlayerState>,
    clock: Arc<dyn Clock>,
}

impl ProtocolClientBuilder {
    /// Create a new builder
    pub fn builder() -> ProtocolClientBuilderFieldsBuilder {
        ProtocolClientBuilderFields::builder()
    }

    /// Get the supported roles that will be sent in the client hello
    pub fn supported_roles(&self) -> &[String] {
        &self.supported_roles
    }

    /// Get the player v1 support configuration
    pub fn player_v1_support(&self) -> Option<&PlayerV1Support> {
        self.player_v1_support.as_ref()
    }

    /// Override the default clock with a custom implementation.
    ///
    /// By default, the builder uses [`DefaultClock`] which reads
    /// `CLOCK_MONOTONIC_RAW` on Linux (immune to NTP slew) and the
    /// platform's native raw monotonic source elsewhere. Override this
    /// for testing or for platforms with alternative high-precision clocks.
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Connect to Sendspin server.
    ///
    /// Accepts anything that implements [`IntoClientRequest`], such as a URL string
    /// for simple connections. For custom headers (for example, auth cookies), callers
    /// will typically build an `http::Request<()>` — see the [`IntoClientRequest`] docs
    /// for the full set of supported request types.
    pub async fn connect<R: IntoClientRequest + Unpin>(
        self,
        request: R,
    ) -> Result<ProtocolClient, Error> {
        let config = self.into_config()?;
        ProtocolClient::connect(request, config).await
    }

    /// Adopt an already-handshaked WebSocket stream and drive the protocol
    /// from `client/init` onwards.
    ///
    /// Use this when you're routing by HTTP path or otherwise need to own the
    /// WebSocket layer yourself. For the common "bind a TCP socket and accept
    /// inbound peers" case, use [`Self::listen`].
    pub async fn accept<S>(self, ws_stream: WebSocketStream<S>) -> Result<ProtocolClient, Error>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let config = self.into_config()?;
        ProtocolClient::drive(ws_stream, config).await
    }

    /// Bind a TCP listener and produce a [`ProtocolListener`] that accepts
    /// inbound WebSocket peers. The builder is cloned per accepted peer.
    pub async fn listen<A: ToSocketAddrs>(self, addr: A) -> Result<ProtocolListener, Error> {
        let tcp = TcpListener::bind(addr)
            .await
            .map_err(|e| Error::Connection(format!("TCP bind failed: {e}")))?;
        let local = tcp
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        log::info!("ProtocolListener bound on {local}");
        Ok(ProtocolListener::new(tcp, self))
    }

    pub(crate) fn into_config(self) -> Result<SessionConfig, Error> {
        let identity = Arc::new(self.credentials.identity().clone());
        let pairing_psk = self.credentials.pairing_psk().clone();

        if self.max_encoded_artwork_transfer_bytes == 0 {
            return Err(Error::Protocol(
                "max_encoded_artwork_transfer_bytes must be greater than zero".to_string(),
            ));
        }

        // The default store is created when the builder is finalized, so this
        // Arc is shared by every listener clone and accepted connection.
        let store = self
            .pairing_store
            .expect("pairing store is resolved when the builder is finalized");

        // The visualizer role's stream configuration lives in the
        // `client/state` visualizer object; the server streams nothing until
        // it has one, so require it up front.
        if self.visualizer_v1_support.is_some() && self.visualizer_state.is_none() {
            return Err(Error::Protocol(
                "visualizer_v1_support requires visualizer_state (types, rate_max)".to_string(),
            ));
        }
        if self.visualizer_state.is_some() && self.visualizer_v1_support.is_none() {
            return Err(Error::Protocol(
                "visualizer_state requires visualizer_v1_support".to_string(),
            ));
        }
        if let Some(state) = self.visualizer_state.as_ref() {
            state
                .validate()
                .map_err(|m| Error::Protocol(m.to_string()))?;
        }
        if let Some(state) = self.artwork_state.as_ref() {
            state
                .validate()
                .map_err(|m| Error::Protocol(m.to_string()))?;
        }

        // Assemble PSK candidates: the Sentinel, mandatory Pairing PSK, stored
        // records, and any explicitly supplied extras.
        let mut psk_candidates = candidates_from(&store, Some(&pairing_psk));
        psk_candidates.extend(self.psk_records);
        let supported_pair_methods = SupportedPairMethods {
            pairing_psk: Some(Default::default()),
            static_pairing_code: None,
            dynamic_pairing_code: None,
        };

        let hello = HelloTemplate {
            name: self.name,
            device_info: Some(DeviceInfo {
                product_name: self.product_name,
                manufacturer: Some(self.manufacturer.unwrap_or_else(|| "Sendspin".to_string())),
                software_version: self.software_version,
                mac_address: self.mac_address,
            }),
            supported_roles: self.supported_roles,
            player_v1_support: self.player_v1_support.clone(),
            source_v1_support: self.source_v1_support,
            visualizer_v1_support: self.visualizer_v1_support,
            supported_pair_methods,
        };

        // A player client must include the player object in its initial
        // client/state (the server sends no audio or commands before it):
        // seed a settable-volume/mute default when none was provided.
        let player_state = self.initial_player_state.or_else(|| {
            self.player_v1_support.is_some().then(|| PlayerState {
                volume: Some(100),
                muted: Some(false),
                output_delay_ms: 0,
                required_lead_time_ms: 0,
                min_buffer_ms: 0,
                supported_commands: vec![PlayerStateCommand::Volume, PlayerStateCommand::Mute],
                format: None,
            })
        });
        if player_state.is_some() && self.player_v1_support.is_none() {
            return Err(Error::Protocol(
                "initial_player_state requires player_v1_support".to_string(),
            ));
        }
        if let Some(state) = player_state.as_ref() {
            state
                .validate()
                .map_err(|message| Error::Protocol(message.to_string()))?;
            if let Some(format) = state.format.as_ref() {
                let supported = self
                    .player_v1_support
                    .as_ref()
                    .is_some_and(|support| support.supported_formats.contains(format));
                if !supported {
                    return Err(Error::Protocol(
                        "initial player format is not in supported_formats".to_string(),
                    ));
                }
            }
        }

        let initial_state = ClientState {
            available: self.initial_available,
            player: player_state,
            source: None,
            artwork: self.artwork_state,
            visualizer: self.visualizer_state,
        };

        Ok(SessionConfig {
            identity,
            suite: self.suite,
            psk_candidates,
            store,
            unpaired_access: self.unpaired_access,
            hello,
            initial_state,
            max_encoded_artwork_transfer_bytes: self.max_encoded_artwork_transfer_bytes,
            clock: self.clock,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::crypto::{Identity, Psk};
    use crate::protocol::pairing::PairingRecord;

    #[test]
    fn default_pairing_store_is_shared_by_builder_clones() {
        let builder = ProtocolClientBuilder::builder()
            .credentials(ClientCredentials::from_parts(
                Identity::from_secret_bytes([1u8; 32]),
                Psk::new([2u8; 32]),
            ))
            .name("test".to_string())
            .build();
        let clone = builder.clone();
        let first = builder.into_config().unwrap();
        let second = clone.into_config().unwrap();

        first
            .store
            .add_record(PairingRecord {
                psk: Psk::new([3u8; 32]),
                server_id: "server".to_string(),
                used: false,
            })
            .unwrap();
        assert_eq!(second.store.records().len(), 1);
    }
}
