// ABOUTME: Binary role-data codecs: audio, artwork-transfer, and visualizer parsing
// ABOUTME: Maps binary message type IDs to typed chunks per the Sendspin spec

//! Binary role-data messages.
//!
//! After AEAD decryption, the first byte of every binary message is its
//! message ID. This module maps the role-owned ID ranges (player 4-7,
//! artwork 8-11, source 12-15, visualizer 16-23) to typed chunks, and
//! implements the artwork announce/part/cancel transfer protocol.

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
    /// Microseconds from the server's transmission of this chunk to
    /// `timestamp`, saturating: `0` and `u32::MAX` report that no lead was
    /// measured. Use [`Self::send_ahead_us`] for delay measurement.
    pub send_ahead: u32,
    /// Raw audio data bytes
    pub data: Arc<[u8]>,
}

impl AudioChunk {
    /// Parse from WebSocket binary frame (type 4 = player audio)
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.len() < 13 {
            return Err(Error::Protocol(format!(
                "Audio chunk too short: got {} bytes, need at least 13",
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
        let send_ahead = u32::from_be_bytes([frame[9], frame[10], frame[11], frame[12]]);
        let data = Arc::from(&frame[13..]);

        Ok(Self {
            timestamp,
            send_ahead,
            data,
        })
    }

    /// The measured send-ahead, usable as an arrival-delay sample: the
    /// server's transmit time is `timestamp - send_ahead` in the server's
    /// clock. Returns `None` for the saturation values (`0` and `u32::MAX`),
    /// which report that no lead was measured, not a lead of that length.
    ///
    /// `send_ahead` carries no scheduling meaning and must not affect when
    /// the chunk is played.
    pub fn send_ahead_us(&self) -> Option<u32> {
        match self.send_ahead {
            0 | u32::MAX => None,
            lead => Some(lead),
        }
    }
}

/// Artwork `flags` bits (byte 1 of an artwork binary message).
mod artwork_flags {
    /// Set on a cancel message.
    pub const CANCEL: u8 = 0b0000_0001;
    /// Set on an announce message.
    pub const ANNOUNCE: u8 = 0b0000_0010;
    /// Bits 2-7 are reserved and must be zero.
    pub const RESERVED: u8 = !(CANCEL | ANNOUNCE);
}

/// An artwork message must fit in a single Noise transport message without
/// fragmentation (spec: artwork binary).
const ARTWORK_MAX_MESSAGE: usize = 65519;
/// Default maximum number of encoded JPEG/PNG bytes accepted in one artwork
/// transfer. This is a local memory-safety policy, not a Sendspin protocol cap.
pub const DEFAULT_MAX_ENCODED_ARTWORK_TRANSFER_BYTES: usize = 16 * 1024 * 1024;

/// Maximum encoded artwork bytes in one transfer, unless overridden by the
/// client builder. This is separate from the fixed per-message protocol limit.
const ARTWORK_MAX_TRANSFER: usize = DEFAULT_MAX_ENCODED_ARTWORK_TRANSFER_BYTES;

/// A completed artwork image transfer.
#[derive(Debug, Clone)]
pub struct ArtworkImage {
    /// Artwork channel (0-3)
    pub channel: u8,
    /// Server clock time in microseconds when the image should be displayed.
    /// A future timestamp schedules the image (at most one pending image per
    /// channel; a newer announce replaces it). Never dropped for lateness.
    pub timestamp: i64,
    /// The encoded image (JPEG or PNG). Empty means clear the channel.
    pub data: Arc<[u8]>,
}

impl ArtworkImage {
    /// Whether this image clears the channel (an announce with `total_size` 0).
    pub fn is_clear(&self) -> bool {
        self.data.is_empty()
    }
}

/// An artwork event delivered to the application.
#[derive(Debug, Clone)]
pub enum ArtworkMessage {
    /// A completed image transfer for a channel. Display (or schedule) it as
    /// the channel's pending image, replacing any held one.
    Image(ArtworkImage),
    /// Discard the channel's pending image; the current image is unaffected.
    Cancel {
        /// Artwork channel (0-3)
        channel: u8,
    },
}

/// One in-flight artwork transfer.
#[derive(Debug)]
struct ArtworkTransfer {
    channel: u8,
    timestamp: i64,
    total_size: usize,
    buffer: Vec<u8>,
}

/// Receiver state machine for the artwork announce/part/cancel protocol.
///
/// At most one image transfer is in flight at a time across all channels.
/// Malformed sequences are protocol errors: the caller MUST close the
/// connection on `Err`.
#[derive(Debug)]
pub(crate) struct ArtworkAssembler {
    in_flight: Option<ArtworkTransfer>,
    max_encoded_transfer_bytes: usize,
}

impl Default for ArtworkAssembler {
    fn default() -> Self {
        Self::new(ARTWORK_MAX_TRANSFER)
    }
}

impl ArtworkAssembler {
    pub(crate) fn new(max_encoded_transfer_bytes: usize) -> Self {
        Self {
            in_flight: None,
            max_encoded_transfer_bytes,
        }
    }

    /// Consume one artwork binary frame (including its leading type byte).
    ///
    /// Returns `Ok(Some(_))` when an event is ready for the application,
    /// `Ok(None)` while a transfer is accumulating, and `Err` on a malformed
    /// sequence (close the connection).
    pub fn handle(&mut self, frame: &[u8]) -> Result<Option<ArtworkMessage>, Error> {
        if frame.len() > ARTWORK_MAX_MESSAGE {
            return Err(Error::Protocol(format!(
                "artwork message exceeds {ARTWORK_MAX_MESSAGE} bytes"
            )));
        }
        if frame.len() < 2 {
            return Err(Error::Protocol(
                "artwork message shorter than 2 bytes".to_string(),
            ));
        }
        let channel = binary_types::artwork_channel(frame[0])
            .ok_or_else(|| Error::Protocol(format!("invalid artwork type {}", frame[0])))?;
        let flags = frame[1];
        if flags & artwork_flags::RESERVED != 0 {
            return Err(Error::Protocol(format!(
                "nonzero reserved artwork flag bits: {flags:#010b}"
            )));
        }
        let cancel = flags & artwork_flags::CANCEL != 0;
        let announce = flags & artwork_flags::ANNOUNCE != 0;
        if cancel && announce {
            return Err(Error::Protocol(
                "artwork message with both cancel and announce bits set".to_string(),
            ));
        }

        if cancel {
            if frame.len() != 2 {
                return Err(Error::Protocol(
                    "artwork cancel message longer than 2 bytes".to_string(),
                ));
            }
            // A cancel for the in-flight transfer's channel discards it.
            if self
                .in_flight
                .as_ref()
                .is_some_and(|t| t.channel == channel)
            {
                self.in_flight = None;
            }
            return Ok(Some(ArtworkMessage::Cancel { channel }));
        }

        if announce {
            if frame.len() != 14 {
                return Err(Error::Protocol(format!(
                    "artwork announce must be 14 bytes, got {}",
                    frame.len()
                )));
            }
            if self.in_flight.is_some() {
                return Err(Error::Protocol(
                    "artwork announce while a transfer is in flight".to_string(),
                ));
            }
            let timestamp = i64::from_be_bytes([
                frame[2], frame[3], frame[4], frame[5], frame[6], frame[7], frame[8], frame[9],
            ]);
            let total_size =
                u32::from_be_bytes([frame[10], frame[11], frame[12], frame[13]]) as usize;
            if total_size == 0 {
                // An empty image completes immediately (clears the channel).
                return Ok(Some(ArtworkMessage::Image(ArtworkImage {
                    channel,
                    timestamp,
                    data: Arc::from(&[][..]),
                })));
            }
            if total_size > self.max_encoded_transfer_bytes {
                return Err(Error::Protocol(format!(
                    "encoded artwork transfer of {total_size} bytes exceeds configured maximum of {} bytes",
                    self.max_encoded_transfer_bytes
                )));
            }
            self.in_flight = Some(ArtworkTransfer {
                channel,
                timestamp,
                total_size,
                // Do not reserve the peer-declared total size. Parts are
                // bounded individually and the aggregate announce is capped.
                buffer: Vec::new(),
            });
            return Ok(None);
        }

        // Part.
        let Some(transfer) = self.in_flight.as_mut() else {
            return Err(Error::Protocol(
                "artwork part with no transfer in flight".to_string(),
            ));
        };
        if transfer.channel != channel {
            return Err(Error::Protocol(
                "artwork part on a channel other than the in-flight transfer's".to_string(),
            ));
        }
        let data = &frame[2..];
        let new_len = transfer
            .buffer
            .len()
            .checked_add(data.len())
            .ok_or_else(|| {
                Error::Protocol("artwork part size overflows the transfer length".to_string())
            })?;
        if new_len > transfer.total_size {
            return Err(Error::Protocol(
                "artwork part data extends past total_size".to_string(),
            ));
        }
        transfer.buffer.extend_from_slice(data);
        if transfer.buffer.len() == transfer.total_size {
            let transfer = self.in_flight.take().expect("in flight");
            return Ok(Some(ArtworkMessage::Image(ArtworkImage {
                channel: transfer.channel,
                timestamp: transfer.timestamp,
                data: Arc::from(transfer.buffer),
            })));
        }
        Ok(None)
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
        let data = &frame[9..];

        // Per-type payload sizes from the visualizer role spec: loudness is
        // one uint16, beat and peak are one uint8, f_peak is two uint16s,
        // and spectrum is one uint16 per display bin (bin count is stream
        // configuration, so only evenness is checkable here).
        let size_ok = match frame[0] {
            binary_types::VISUALIZER_LOUDNESS => data.len() == 2,
            binary_types::VISUALIZER_BEAT | binary_types::VISUALIZER_PEAK => data.len() == 1,
            binary_types::VISUALIZER_F_PEAK => data.len() == 4,
            binary_types::VISUALIZER_SPECTRUM => !data.is_empty() && data.len().is_multiple_of(2),
            _ => unreachable!("checked above"),
        };
        if !size_ok {
            return Err(Error::Protocol(format!(
                "Visualizer chunk type {} has invalid data length {}",
                frame[0],
                data.len()
            )));
        }

        Ok(Self {
            type_id: frame[0],
            timestamp,
            data: Arc::from(data),
        })
    }
}

/// Binary frame from server (any type except artwork, whose stateful
/// transfer protocol is handled internally and surfaced as
/// [`ArtworkMessage`] events)
#[derive(Debug, Clone)]
pub enum BinaryFrame {
    /// Player audio (type 4)
    Audio(AudioChunk),
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
    /// excludes the leading message-type byte. Artwork types (8-11) are not
    /// handled here; the connection router runs their transfer protocol and
    /// delivers [`ArtworkMessage`] events.
    pub fn from_parts(msg_type: u8, payload: &[u8]) -> Result<Self, Error> {
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(msg_type);
        frame.extend_from_slice(payload);
        Self::from_bytes(&frame)
    }

    /// Parse any non-artwork binary frame from WebSocket
    pub fn from_bytes(frame: &[u8]) -> Result<Self, Error> {
        if frame.is_empty() {
            return Err(Error::Protocol("Empty binary frame".to_string()));
        }

        let type_id = frame[0];

        match type_id {
            binary_types::PLAYER_AUDIO => Ok(BinaryFrame::Audio(AudioChunk::from_bytes(frame)?)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn announce(channel: u8, timestamp: i64, total_size: u32) -> Vec<u8> {
        let mut frame = vec![
            binary_types::ARTWORK_CHANNEL_0 + channel,
            artwork_flags::ANNOUNCE,
        ];
        frame.extend_from_slice(&timestamp.to_be_bytes());
        frame.extend_from_slice(&total_size.to_be_bytes());
        frame
    }

    fn part(channel: u8, data: &[u8]) -> Vec<u8> {
        let mut frame = vec![binary_types::ARTWORK_CHANNEL_0 + channel, 0];
        frame.extend_from_slice(data);
        frame
    }

    #[test]
    fn artwork_assembles_multipart_transfer() {
        let mut assembler = ArtworkAssembler::default();
        assert!(assembler.handle(&announce(1, 42, 5)).unwrap().is_none());
        assert!(assembler.handle(&part(1, b"ab")).unwrap().is_none());
        let Some(ArtworkMessage::Image(image)) = assembler.handle(&part(1, b"cde")).unwrap() else {
            panic!("expected completed image");
        };
        assert_eq!(image.channel, 1);
        assert_eq!(image.timestamp, 42);
        assert_eq!(&*image.data, b"abcde");
    }

    #[test]
    fn artwork_custom_limit_counts_encoded_bytes_across_parts() {
        let mut assembler = ArtworkAssembler::new(5);
        assert!(assembler.handle(&announce(0, 0, 5)).unwrap().is_none());
        assert!(assembler.handle(&part(0, b"abc")).unwrap().is_none());
        let Some(ArtworkMessage::Image(image)) = assembler.handle(&part(0, b"de")).unwrap() else {
            panic!("expected completed image");
        };
        assert_eq!(&*image.data, b"abcde");
    }

    #[test]
    fn artwork_custom_limit_rejects_announce_before_transfer_state() {
        let mut assembler = ArtworkAssembler::new(4);
        let error = assembler
            .handle(&announce(0, 0, 5))
            .expect_err("over-limit artwork accepted");
        assert!(error.to_string().contains("configured maximum of 4 bytes"));
        assert!(assembler.handle(&part(0, b"data")).is_err());
    }

    #[test]
    fn artwork_custom_limit_can_exceed_default_without_allocating() {
        let mut assembler = ArtworkAssembler::new(ARTWORK_MAX_TRANSFER + 1);
        assert!(assembler
            .handle(&announce(0, 0, (ARTWORK_MAX_TRANSFER + 1) as u32))
            .unwrap()
            .is_none());
        assert!(matches!(
            assembler.handle(&[binary_types::ARTWORK_CHANNEL_0, artwork_flags::CANCEL]),
            Ok(Some(ArtworkMessage::Cancel { channel: 0 }))
        ));
    }

    #[test]
    fn artwork_zero_size_announce_clears_immediately() {
        let mut assembler = ArtworkAssembler::default();
        let Some(ArtworkMessage::Image(image)) = assembler.handle(&announce(0, -7, 0)).unwrap()
        else {
            panic!("expected clear image");
        };
        assert!(image.is_clear());
        assert_eq!(image.timestamp, -7);
    }

    #[test]
    fn artwork_cancel_discards_in_flight_transfer() {
        let mut assembler = ArtworkAssembler::default();
        assert!(assembler.handle(&announce(2, 0, 3)).unwrap().is_none());
        assert!(matches!(
            assembler.handle(&[binary_types::ARTWORK_CHANNEL_0 + 2, artwork_flags::CANCEL]),
            Ok(Some(ArtworkMessage::Cancel { channel: 2 }))
        ));
        assert!(assembler.handle(&part(2, b"abc")).is_err());
    }

    #[test]
    fn artwork_rejects_malformed_sequences() {
        let mut assembler = ArtworkAssembler::default();
        for frame in [
            vec![],
            vec![binary_types::ARTWORK_CHANNEL_0],
            vec![binary_types::ARTWORK_CHANNEL_0, artwork_flags::RESERVED],
            vec![
                binary_types::ARTWORK_CHANNEL_0,
                artwork_flags::CANCEL | artwork_flags::ANNOUNCE,
            ],
            vec![binary_types::ARTWORK_CHANNEL_0, artwork_flags::CANCEL, 0],
            vec![binary_types::ARTWORK_CHANNEL_0, artwork_flags::ANNOUNCE, 0],
            part(0, b"data"),
        ] {
            assert!(assembler.handle(&frame).is_err(), "accepted {frame:?}");
        }
    }

    #[test]
    fn artwork_rejects_wrong_channel_overrun_and_overlap() {
        let mut assembler = ArtworkAssembler::default();
        assert!(assembler.handle(&announce(0, 0, 2)).unwrap().is_none());
        assert!(assembler.handle(&part(1, b"x")).is_err());
        assert!(assembler.handle(&part(0, b"xyz")).is_err());
        assert!(assembler.handle(&announce(0, 0, 1)).is_err());
    }

    #[test]
    fn artwork_rejects_huge_aggregate_transfer_without_allocating_it() {
        let mut assembler = ArtworkAssembler::default();
        let frame = announce(0, 0, u32::MAX);
        let error = assembler
            .handle(&frame)
            .expect_err("huge transfer accepted");
        assert!(error.to_string().contains("encoded artwork transfer of"));
    }

    #[test]
    fn artwork_rejects_oversized_single_message() {
        let mut assembler = ArtworkAssembler::default();
        let mut frame = vec![0; ARTWORK_MAX_MESSAGE + 1];
        frame[0] = binary_types::ARTWORK_CHANNEL_0;
        assert!(assembler.handle(&frame).is_err());
    }
}
