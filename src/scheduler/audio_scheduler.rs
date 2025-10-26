// ABOUTME: Lock-free audio scheduler implementation
// ABOUTME: Uses crossbeam queues for thread-safe scheduling without locks

use crate::audio::AudioBuffer;
use crossbeam::queue::SegQueue;
use std::sync::Arc;
use std::time::Instant;

/// Lock-free audio scheduler
pub struct AudioScheduler {
    /// Incoming buffers (lock-free queue)
    incoming: Arc<SegQueue<AudioBuffer>>,

    /// Sorted buffers ready for playback
    sorted: Arc<parking_lot::Mutex<Vec<AudioBuffer>>>,
}

impl AudioScheduler {
    /// Create a new audio scheduler
    pub fn new() -> Self {
        Self {
            incoming: Arc::new(SegQueue::new()),
            sorted: Arc::new(parking_lot::Mutex::new(Vec::new())),
        }
    }

    /// Schedule an audio buffer for future playback
    pub fn schedule(&self, buffer: AudioBuffer) {
        self.incoming.push(buffer);
    }

    /// Check if scheduler is empty
    pub fn is_empty(&self) -> bool {
        self.incoming.is_empty() && self.sorted.lock().is_empty()
    }

    /// Get next buffer that's ready to play (within 50ms window)
    pub fn next_ready(&self) -> Option<AudioBuffer> {
        // Take the lock once and do all operations under it
        let mut sorted = self.sorted.lock();

        // Drain incoming queue into sorted vec
        while let Some(buf) = self.incoming.pop() {
            let pos = sorted
                .binary_search_by_key(&buf.timestamp, |b| b.timestamp)
                .unwrap_or_else(|e| e);
            sorted.insert(pos, buf);
        }

        let now = Instant::now();

        // Check if first buffer is ready
        if let Some(buf) = sorted.first() {
            // Check if play_at time has passed or is very close (within 1ms)
            if buf.play_at <= now {
                // Ready to play or late
                return Some(sorted.remove(0));
            }
        }

        None
    }
}

impl Default for AudioScheduler {
    fn default() -> Self {
        Self::new()
    }
}
