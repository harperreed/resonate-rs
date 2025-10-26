// ABOUTME: Audio chunk scheduler for timed playback
// ABOUTME: Lock-free priority queue for scheduling audio buffers

/// Audio scheduler implementation
pub mod audio_scheduler;

pub use audio_scheduler::AudioScheduler;
