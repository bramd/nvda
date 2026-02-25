pub mod player;
pub mod device;
pub mod silence;
pub mod silence_detect;

/// 400ms buffer, matching C++ BUFFER_MS
pub const BUFFER_MS: u32 = 400;
/// REFERENCE_TIME units per millisecond
pub const REFTIMES_PER_MILLISEC: i64 = 10_000;
/// Buffer size in REFERENCE_TIME units
pub const BUFFER_SIZE: i64 = BUFFER_MS as i64 * REFTIMES_PER_MILLISEC;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Stopping,
}
