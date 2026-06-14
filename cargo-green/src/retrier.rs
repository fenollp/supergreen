use std::time::Duration;

use log::warn;
use tokio::time::sleep;

pub(crate) struct Retrier {
    max: u8,
    attempt: u8,
}

impl Retrier {
    pub(crate) fn with_max_attempts(max: u8) -> Self {
        Self { max, attempt: 0 }
    }

    /// Returns true when the maximum number of attempts is not reached
    #[must_use]
    pub(crate) fn continues(&self) -> bool {
        self.attempt < self.max
    }

    /// Bumps attempts, sleeps and logs
    pub(crate) async fn backoff(&mut self, stamp: &'static str, e: anyhow::Error) {
        warn!("spurious {stamp} error: {e}");

        let secs = 1u64 << self.attempt; // exponential
        self.attempt += 1;

        warn!("hit a transient error, retrying in {secs}s ({}/{})", self.attempt, self.max);
        sleep(Duration::from_secs(secs)).await;
    }
}
