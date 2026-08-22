// ABOUTME: Protocol implementation for Sendspin WebSocket protocol
// ABOUTME: Message types, serialization, and WebSocket client

/// Binary role-data codecs (audio, artwork, visualizer chunks)
pub mod binary;
/// WebSocket client: establishment, task supervision, frame routing
pub mod client;
/// Builder for easy construction of the client
pub mod client_builder;
/// Cryptographic foundation: identities, PSKs, and the Noise KKpsk2 layer
pub mod crypto;
/// Inbound WebSocket acceptor for server-initiated connections
pub mod listener;
/// Client-side handling of management requests
pub(crate) mod management;
/// Managed connection lifecycle: multi-server arbitration and auto-goodbye
pub mod manager;
/// Protocol message type definitions and serialization
pub mod messages;
/// Pairing tokens and the pairing record store
pub mod pairing;
/// Role facades (WsSender, Controller, Source) over an established session
pub mod roles;
/// Session admission rules and the session protocol state machine
pub(crate) mod session;
/// Noise transport layer: handshake driver, encrypted channel, fragmentation
pub mod transport;
/// The single outbound writer task for a connection
pub(crate) mod writer;

pub use client::{Connection, ConnectionGuard, Controller, Source, WsSender};
pub use listener::ProtocolListener;
pub use manager::{
    should_switch, ArbitrationState, ConnectionManager, ManagedConnection, ManagerConfig,
};
pub use messages::Message;
