// ABOUTME: Main library entry point for resonate-rs
// ABOUTME: Exports public API for Resonate Protocol client and server

#![warn(missing_docs)]

pub mod audio;
// pub mod protocol;
// pub mod sync;

// pub use protocol::client::ProtocolClient;
// pub use protocol::messages::{ClientHello, ServerHello};

/// Result type for resonate operations
pub type Result<T> = std::result::Result<T, error::Error>;

/// Error types for resonate
pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum Error {
        #[error("WebSocket error: {0}")]
        WebSocket(String),

        #[error("Protocol error: {0}")]
        Protocol(String),

        #[error("Invalid message format")]
        InvalidMessage,

        #[error("Connection error: {0}")]
        Connection(String),
    }
}
