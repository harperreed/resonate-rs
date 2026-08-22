// ABOUTME: Builder exposed for public usage of the library
// ABOUTME: Assembles identity, PSK candidates, and hello capabilities into a SessionConfig

use crate::error::Error;
use crate::protocol::client::SessionConfig;
use crate::protocol::crypto::{CipherSuite, Identity, Psk, PskCandidate};
use crate::protocol::listener::ProtocolListener;
use crate::protocol::messages::{
    ArtworkV1Support, AudioFormatSpec, ClientState, DeviceInfo, PairMethodDescriptor,
    PairingMethod, PlayerState, PlayerV1Support, SourceV1Support, VisualizerV1Support,
};
use crate::protocol::pairing::{candidates_from, MemoryPairingStore, PairingRecord, PairingStore};
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
    identity: Option<Arc<Identity>>,
    suite: CipherSuite,
    pairing_psk: Option<Psk>,
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
    artwork_v1_support: Option<ArtworkV1Support>,
    visualizer_v1_support: Option<VisualizerV1Support>,
    initial_available: bool,
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
            || raw.artwork_v1_support.is_some()
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
                supported_commands: vec!["volume".to_string(), "mute".to_string()],
            })
        };

        if player_v1_support.is_some() {
            supported_roles.push("player@v1".to_string());
        }
        if raw.source_v1_support.is_some() {
            supported_roles.push("source@v1".to_string());
        }
        if raw.artwork_v1_support.is_some() {
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
            identity: raw.identity,
            suite: raw.suite,
            pairing_psk: raw.pairing_psk,
            psk_records: raw.psk_records,
            pairing_store: raw.pairing_store,
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
            artwork_v1_support: raw.artwork_v1_support,
            visualizer_v1_support: raw.visualizer_v1_support,
            initial_available: raw.initial_available,
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
    /// The client's static Curve25519 identity. Persist and reuse the secret
    /// key across restarts so servers recognize this client; a fresh identity
    /// is generated when omitted.
    #[builder(default = None, setter(transform = |x: Identity| Some(Arc::new(x))))]
    identity: Option<Arc<Identity>>,
    /// The Noise cipher suite announced in `client/init`.
    #[builder(default = CipherSuite::ChaChaPoly)]
    suite: CipherSuite,
    /// The client's Pairing PSK: offering it advertises the `pairing_psk`
    /// method and keeps it among the handshake PSK candidates.
    #[builder(default = None, setter(transform = |x: Psk| Some(x)))]
    pairing_psk: Option<Psk>,
    /// Long-term pairing-record PSK candidates (stored-pubkey or shared-PSK).
    #[builder(default = Vec::new())]
    psk_records: Vec<PskCandidate>,
    /// Persistence for pairing records: existing records become handshake
    /// candidates and newly paired records are written here. Defaults to an
    /// in-memory store that does not survive restarts.
    #[builder(default = None, setter(transform = |x: Arc<dyn PairingStore>| Some(x)))]
    pairing_store: Option<Arc<dyn PairingStore>>,
    /// Whether this client admits unpaired (Sentinel-PSK) playback sessions
    /// at trust level `none`. Defaults to `true` for frictionless setups;
    /// products handling sensitive inputs should disable it and pair.
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
    #[builder(default = None, setter(transform = |x: ArtworkV1Support| Some(x)))]
    artwork_v1_support: Option<ArtworkV1Support>,
    #[builder(default = None, setter(transform = |x: VisualizerV1Support| Some(x)))]
    visualizer_v1_support: Option<VisualizerV1Support>,
    /// Initial top-level availability sent in the first `client/state`.
    /// `false` when another source owns the output at connect time.
    #[builder(default = true)]
    initial_available: bool,
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
            identity: fields.identity,
            suite: fields.suite,
            pairing_psk: fields.pairing_psk,
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
            artwork_v1_support: fields.artwork_v1_support,
            visualizer_v1_support: fields.visualizer_v1_support,
            initial_available: fields.initial_available,
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
    identity: Option<Arc<Identity>>,
    suite: CipherSuite,
    pairing_psk: Option<Psk>,
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
    artwork_v1_support: Option<ArtworkV1Support>,
    visualizer_v1_support: Option<VisualizerV1Support>,
    initial_available: bool,
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
        let identity = match self.identity {
            Some(identity) => identity,
            None => Arc::new(Identity::generate()?),
        };

        let store = self
            .pairing_store
            .unwrap_or_else(|| Arc::new(MemoryPairingStore::new()));

        // Ensure a shared-PSK record exists for `record_mode` (the spec's
        // pre-provisioned storage-exhaustion fallback; management.md#record-mode).
        // Done before candidate assembly so the record is a handshake candidate.
        let record_mode = match store.records().iter().find(|r| r.server_id.is_none()) {
            Some(shared) => shared.psk_id(),
            None => {
                let record = PairingRecord {
                    psk: Psk::generate()?,
                    server_id: None,
                    used: false,
                };
                let psk_id = record.psk_id();
                if let Err(e) = store.add_record(record) {
                    // Degenerate (e.g. exhausted app store): the id still
                    // names the intended fallback, but it isn't persisted.
                    log::error!("Failed to pre-provision shared-PSK record: {e:?}");
                }
                psk_id
            }
        };

        // Assemble PSK candidates: the Sentinel is always a candidate, the
        // Pairing PSK when configured, plus stored records and any extras.
        let mut psk_candidates = candidates_from(&store, self.pairing_psk.as_ref());
        psk_candidates.extend(self.psk_records);
        let pairing_psk = self.pairing_psk;
        let mut supported_pair_methods = Vec::new();
        if pairing_psk.is_some() {
            supported_pair_methods.push(PairMethodDescriptor {
                method: PairingMethod::PairingPsk,
                out_channels: None,
                formats: None,
                locations: None,
            });
        }

        let hello = HelloTemplate {
            name: self.name,
            device_info: Some(DeviceInfo {
                product_name: self.product_name,
                manufacturer: Some(self.manufacturer.unwrap_or_else(|| "Sendspin".to_string())),
                software_version: self.software_version,
                mac_address: self.mac_address,
            }),
            supported_roles: self.supported_roles,
            player_v1_support: self.player_v1_support,
            source_v1_support: self.source_v1_support,
            artwork_v1_support: self.artwork_v1_support,
            visualizer_v1_support: self.visualizer_v1_support,
            supported_pair_methods,
        };

        let initial_state = ClientState {
            available: self.initial_available,
            player: self.initial_player_state,
            source: None,
        };

        Ok(SessionConfig {
            identity,
            suite: self.suite,
            psk_candidates,
            store,
            pairing_psk,
            unpaired_access: self.unpaired_access,
            record_mode,
            hello,
            initial_state,
            clock: self.clock,
        })
    }
}
