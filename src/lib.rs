// ABOUTME: Main library entry point for resonate-rs
// ABOUTME: Exports public API for Resonate Protocol client and server

//! # resonate-rs
//!
//! Hyper-efficient Rust implementation of the Resonate Protocol for synchronized multi-room audio streaming.
//!
//! This library provides zero-copy audio pipelines, lock-free concurrency, and async I/O
//! for building high-performance audio streaming clients and servers.

#![warn(missing_docs)]

/// Audio types and processing
pub mod audio;
/// Protocol implementation for WebSocket communication
pub mod protocol;
/// Clock synchronization utilities
pub mod sync;
/// Audio scheduler for timed playback
pub mod scheduler;

pub use protocol::client::ProtocolClient;
pub use protocol::messages::{ClientHello, ServerHello};
pub use scheduler::AudioScheduler;

/// Result type for resonate operations
pub type Result<T> = std::result::Result<T, error::Error>;

/// Error types for resonate
pub mod error {
    use thiserror::Error;

    /// Error types for resonate operations
    #[derive(Error, Debug)]
    pub enum Error {
        /// WebSocket-related error
        #[error("WebSocket error: {0}")]
        WebSocket(String),

        /// Protocol violation or parsing error
        #[error("Protocol error: {0}")]
        Protocol(String),

        /// Invalid message format received
        #[error("Invalid message format")]
        InvalidMessage,

        /// Connection-related error
        #[error("Connection error: {0}")]
        Connection(String),

        /// Audio output error
        #[error("Audio output error: {0}")]
        Output(String),
    }
}
