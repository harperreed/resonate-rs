// ABOUTME: Protocol implementation for Resonate WebSocket protocol
// ABOUTME: Message types, serialization, and WebSocket client

/// WebSocket client implementation
pub mod client;
/// Protocol message type definitions and serialization
pub mod messages;

pub use messages::Message;
