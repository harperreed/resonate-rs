// ABOUTME: Audio types and processing for resonate-rs
// ABOUTME: Contains Sample type, AudioFormat, Buffer, and codec definitions

/// Audio decoder implementations (PCM, Opus, FLAC)
pub mod decode;
/// Core audio type definitions (Sample, Codec, AudioFormat, AudioBuffer)
pub mod types;

pub use types::{AudioBuffer, AudioFormat, Codec, Sample};
