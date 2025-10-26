// ABOUTME: Audio types and processing for resonate-rs
// ABOUTME: Contains Sample type, AudioFormat, Buffer, and codec definitions

pub mod types;
pub mod decode;

pub use types::{Sample, Codec, AudioFormat, AudioBuffer};
