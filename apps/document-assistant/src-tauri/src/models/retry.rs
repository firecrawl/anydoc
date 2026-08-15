use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub maximum_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(50),
            maximum_delay: Duration::from_secs(10),
        }
    }
}

impl RetryPolicy {
    pub fn should_retry_status(status: u16) -> bool {
        matches!(status, 429 | 500 | 502 | 503 | 520)
    }

    pub fn delay(self, attempt: u32, retry_after: Option<&str>) -> Duration {
        if let Some(seconds) = retry_after.and_then(|value| value.trim().parse::<u64>().ok()) {
            return Duration::from_secs(seconds).min(self.maximum_delay);
        }
        let multiplier = 1_u32.checked_shl(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
        let base = self.base_delay.saturating_mul(multiplier).min(self.maximum_delay);
        let jitter_ceiling = (base.as_millis() as u64 / 4).max(1);
        let seed =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos() as u64;
        base.saturating_add(Duration::from_millis(seed % jitter_ceiling)).min(self.maximum_delay)
    }
}
