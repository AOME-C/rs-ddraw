//! FPS limiter.
//!
//! Controls rendering frame rate and game tick speed to prevent
//! old games from running too fast on modern hardware.

pub(crate) struct FpsLimiter {
    pub target_fps: i32,
}

impl FpsLimiter {
    pub fn new(target_fps: i32) -> Self {
        Self { target_fps }
    }

    /// Wait until the next frame is due.
    pub fn wait(&self) {
        // TODO: implement timing with timeBeginPeriod / Sleep / spin
    }
}
