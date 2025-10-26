// ABOUTME: Audio decoder implementations
// ABOUTME: PCM, Opus, FLAC decoders (Phase 1: PCM only)

pub mod pcm;

pub use pcm::PcmDecoder;

use crate::audio::Sample;
use crate::error::Error;
use std::sync::Arc;

/// Decoder trait for audio codecs
pub trait Decoder {
    fn decode(&self, data: &[u8]) -> Result<Arc<[Sample]>, Error>;
}
