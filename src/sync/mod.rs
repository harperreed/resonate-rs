// ABOUTME: Clock synchronization for Resonate protocol
// ABOUTME: NTP-style round-trip time calculation and server timestamp conversion

pub mod clock;

pub use clock::{ClockSync, SyncQuality};
