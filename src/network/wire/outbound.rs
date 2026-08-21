//! Wire-side outbound constants and the chat rate limiter.
//! Socket delivery lives in `crate::network::outbound` so this module does
//! not depend on the listener adapter.

pub const OUTBOUND_QUEUE_CAPACITY: usize = 2048;

pub(crate) const BLOCK_SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6);
pub(crate) const HEALTH_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
pub(crate) const MAX_SNAPSHOT_SIZE: usize = 800;
pub(crate) const SLOW_CONSUMER_DROP_LIMIT: u64 = 4096;

#[derive(Clone, Debug)]
pub struct ChatRateLimiter {
    last: std::time::Instant,
    count: u32,
}

impl Default for ChatRateLimiter {
    fn default() -> Self {
        ChatRateLimiter {
            last: std::time::Instant::now(),
            count: 0,
        }
    }
}

impl ChatRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(&mut self, window_ms: u64, limit: u32) -> bool {
        let now = std::time::Instant::now();
        if now.duration_since(self.last).as_millis() as u64 >= window_ms {
            self.last = now;
            self.count = 0;
        }
        if self.count >= limit {
            return false;
        }
        self.count += 1;
        true
    }
}
