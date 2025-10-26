// ABOUTME: Audio types and processing for resonate-rs
// ABOUTME: Contains Sample type, AudioFormat, Buffer, and codec definitions

/// Audio decoder implementations (PCM, Opus, FLAC)
pub mod decode;
/// Audio output trait and implementations
pub mod output;
/// Buffer pool for reusing audio sample buffers
pub mod pool;
/// Core audio type definitions (Sample, Codec, AudioFormat, AudioBuffer)
pub mod types;

pub use output::{AudioOutput, CpalOutput};
pub use pool::BufferPool;
pub use types::{AudioBuffer, AudioFormat, Codec, Sample};
