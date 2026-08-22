// ABOUTME: Binary role-data codecs: audio, artwork, and visualizer chunk parsing
// ABOUTME: Maps binary message type IDs to typed chunks per the Sendspin spec

//! Binary role-data messages.
//!
//! After AEAD decryption, the first byte of every binary message is its
//! message ID. This module maps the role-owned ID ranges (player 4-7,
//! artwork 8-11, source 12-15, visualizer 16-23) to typed chunks.

use crate::error::Error;
use std::sync::Arc;

/// Binary message type IDs per Sendspin spec
pub mod binary_types {
    /// Player audio chunk (types 4-7, we use 4)
    pub const PLAYER_AUDIO: u8 = 0x04;
    /// Source audio chunk, client → server (types 12-15, we use 12)
    pub const SOURCE_AUDIO: u8 = 0x0c;
    /// Artwork channel 0 (type 8)
    pub const ARTWORK_CHANNEL_0: u8 = 0x08;
    /// Artwork channel 1 (type 9)
    pub const ARTWORK_CHANNEL_1: u8 = 0x09;
    /// Artwork channel 2 (type 10)
    pub const ARTWORK_CHANNEL_2: u8 = 0x0A;
    /// Artwork channel 3 (type 11)
    pub const ARTWORK_CHANNEL_3: u8 = 0x0B;
    /// Visualizer loudness data (type 16).
    pub const VISUALIZER_LOUDNESS: u8 = 0x10;
    /// Visualizer beat data (type 17).
    pub const VISUALIZER_BEAT: u8 = 0x11;
    /// Visualizer dominant-frequency data (type 18).
    pub const VISUALIZER_F_PEAK: u8 = 0x12;
    /// Visualizer spectrum data (type 19).
    pub const VISUALIZER_SPECTRUM: u8 = 0x13;
    /// Visualizer energy-onset data (type 20).
    pub const VISUALIZER_PEAK: u8 = 0x14;
    /// Check if a binary type ID is for artwork (8-11)
    pub fn is_artwork(type_id: u8) -> bool {
        (ARTWORK_CHANNEL_0..=ARTWORK_CHANNEL_3).contains(&type_id)
    }

    /// Get artwork channel number from type ID (0-3)
    pub fn artwork_channel(type_id: u8) -> Option<u8> {
        if is_artwork(type_id) {
            Some(type_id - ARTWORK_CHANNEL_0)
        } else {
            None
        }
    }

    /// Check if a binary type ID is for visualizer data (16-20).
    pub fn is_visualizer(type_id: u8) -> bool {
        (VISUALIZER_LOUDNESS..=VISUALIZER_PEAK).contains(&type_id)
    }
}

/// Audio chunk from server (binary type 4)
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Server timestamp in microseconds
    pub timestamp: i64,
    /// Raw audio data bytes
    pub data: Arc<[u8]>,
}

impl AudioChunk {
    /// Parse from WebSocket binary frame (type 4 = player audio)
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < 9 {
            return Err(Error::Protocol(format!(
                "Audio chunk too short: got {} bytes, need at least 9",
                frame.len()
            )));
        }

        // Per spec: player audio uses binary type 4
        if frame[0] != binary_types::PLAYER_AUDIO {
            return Err(Error::Protocol(format!(
                "Invalid audio chunk type: expected {}, got {}",
                binary_types::PLAYER_AUDIO,
                frame[0]
            )));
        }

        let timestamp = i64::from_be_bytes([
            frame[1], frame[2], frame[3], frame[4], frame[5], frame[6], frame[7], frame[8],
        ]);

        let data = Arc::from(&frame[9..]);

        Ok(Self { timestamp, data })
    }
}

/// Artwork chunk from server (binary types 8-11)
#[derive(Debug, Clone)]
pub struct ArtworkChunk {
    /// Artwork channel (0-3)
    pub channel: u8,
    /// Server timestamp in microseconds
    pub timestamp: i64,
    /// Image data bytes (JPEG, PNG, or BMP)
    /// Empty payload means clear the artwork
    pub data: Arc<[u8]>,
}

impl ArtworkChunk {
    /// Parse from WebSocket binary frame (types 8-11 = artwork channels 0-3)
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < 9 {
            return Err(Error::Protocol(format!(
                "Artwork chunk too short: got {} bytes, need at least 9",
                frame.len()
            )));
        }

        let type_id = frame[0];
        let channel = binary_types::artwork_channel(type_id)
            .ok_or_else(|| Error::Protocol(format!("Invalid artwork chunk type: {}", type_id)))?;

        let timestamp = i64::from_be_bytes([
            frame[1], frame[2], frame[3], frame[4], frame[5], frame[6], frame[7], frame[8],
        ]);

        let data = Arc::from(&frame[9..]);

        Ok(Self {
            channel,
            timestamp,
            data,
        })
    }

    /// Check if this is a clear command (empty payload)
    pub fn is_clear(&self) -> bool {
        self.data.is_empty()
    }
}

/// Visualizer chunk from server (binary types 16-20).
#[derive(Debug, Clone)]
pub struct VisualizerChunk {
    /// Visualizer binary message type (16-20).
    pub type_id: u8,
    /// Server timestamp in microseconds.
    pub timestamp: i64,
    /// Raw visualization data bytes, left for the application to decode.
    pub data: Arc<[u8]>,
}

impl VisualizerChunk {
    /// Return the typed visualizer data kind represented by this chunk.
    ///
    /// Returns `None` if a chunk was constructed manually with an invalid
    /// `type_id`; frames parsed by [`Self::from_bytes`] always return `Some`.
    pub fn data_type(&self) -> Option<crate::protocol::messages::VisualizerDataType> {
        use crate::protocol::messages::VisualizerDataType;
        match self.type_id {
            binary_types::VISUALIZER_LOUDNESS => Some(VisualizerDataType::Loudness),
            binary_types::VISUALIZER_BEAT => Some(VisualizerDataType::Beat),
            binary_types::VISUALIZER_F_PEAK => Some(VisualizerDataType::FPeak),
            binary_types::VISUALIZER_SPECTRUM => Some(VisualizerDataType::Spectrum),
            binary_types::VISUALIZER_PEAK => Some(VisualizerDataType::Peak),
            _ => None,
        }
    }

    /// Parse from a WebSocket binary frame (visualizer types 16-20).
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < 9 {
            return Err(Error::Protocol(format!(
                "Visualizer chunk too short: got {} bytes, need at least 9",
                frame.len()
            )));
        }

        if !binary_types::is_visualizer(frame[0]) {
            return Err(Error::Protocol(format!(
                "Invalid visualizer chunk type: expected 16-20, got {}",
                frame[0]
            )));
        }

        let timestamp = i64::from_be_bytes([
            frame[1], frame[2], frame[3], frame[4], frame[5], frame[6], frame[7], frame[8],
        ]);

        let data = Arc::from(&frame[9..]);

        Ok(Self {
            type_id: frame[0],
            timestamp,
            data,
        })
    }
}

/// Binary frame from server (any type)
#[derive(Debug, Clone)]
pub enum BinaryFrame {
    /// Player audio (type 4)
    Audio(AudioChunk),
    /// Artwork image (types 8-11)
    Artwork(ArtworkChunk),
    /// Visualizer data (types 16-20)
    Visualizer(VisualizerChunk),
    /// Unknown binary type
    Unknown {
        /// The unknown type ID
        type_id: u8,
        /// Raw data after the type byte
        data: Arc<[u8]>,
    },
}

impl BinaryFrame {
    /// Parse from a decrypted (msg_type, payload) pair, where `payload`
    /// excludes the leading message-type byte.
    pub fn from_parts(msg_type: u8, payload: &[u8]) -> Result<Self, Error> {
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(msg_type);
        frame.extend_from_slice(payload);
        Self::from_bytes(&frame)
    }

    /// Parse any binary frame from WebSocket
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.is_empty() {
            return Err(Error::Protocol("Empty binary frame".to_string()));
        }

        let type_id = frame[0];

        match type_id {
            binary_types::PLAYER_AUDIO => Ok(BinaryFrame::Audio(AudioChunk::from_bytes(frame)?)),
            t if binary_types::is_artwork(t) => {
                Ok(BinaryFrame::Artwork(ArtworkChunk::from_bytes(frame)?))
            }
            t if binary_types::is_visualizer(t) => {
                Ok(BinaryFrame::Visualizer(VisualizerChunk::from_bytes(frame)?))
            }
            // The router warns when it sees the Unknown variant; parsing
            // itself stays quiet to avoid reporting the same frame twice.
            _ => Ok(BinaryFrame::Unknown {
                type_id,
                data: Arc::from(&frame[1..]),
            }),
        }
    }
}
