// ABOUTME: Core audio type definitions
// ABOUTME: Sample (24-bit), AudioFormat, AudioBuffer for zero-copy audio data

use std::sync::Arc;
use std::time::Instant;

/// 24-bit audio sample stored in i32
/// Range: -8388608 to 8388607 (±2^23)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Sample(pub i32);

impl Sample {
    pub const MAX: Self = Self(8_388_607);   // 2^23 - 1
    pub const MIN: Self = Self(-8_388_608);  // -2^23
    pub const ZERO: Self = Self(0);

    /// Convert from 16-bit sample (shift left 8 bits)
    #[inline]
    pub fn from_i16(s: i16) -> Self {
        Self((s as i32) << 8)
    }

    /// Convert from 24-bit little-endian bytes
    #[inline]
    pub fn from_i24_le(bytes: [u8; 3]) -> Self {
        // Build 24-bit signed integer in i32
        let val = (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
        // Sign-extend from 24-bit to 32-bit
        let extended = if val & 0x00800000 != 0 {
            val | 0xFF000000u32 as i32  // Negative: fill upper 8 bits with 1
        } else {
            val  // Positive: upper 8 bits already 0
        };
        Self(extended)
    }

    /// Convert to 16-bit sample (shift right 8 bits)
    #[inline]
    pub fn to_i16(self) -> i16 {
        (self.0 >> 8) as i16
    }

    /// Clamp to valid 24-bit range
    #[inline]
    pub fn clamp(self) -> Self {
        Self(self.0.clamp(Self::MIN.0, Self::MAX.0))
    }
}

/// Audio codec type
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Codec {
    Pcm,
    Opus,
    Flac,
    Mp3,
}

/// Audio format specification
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub codec: Codec,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
    pub codec_header: Option<Vec<u8>>,
}

/// Audio buffer with timestamp (zero-copy via Arc)
pub struct AudioBuffer {
    pub timestamp: i64,           // Server loop microseconds
    pub play_at: Instant,         // Computed local playback time
    pub samples: Arc<[Sample]>,   // Immutable, shareable sample data
    pub format: AudioFormat,
}
