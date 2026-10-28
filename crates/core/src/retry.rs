//! Retry policy with exponential backoff and jitter.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use crate::error::Error;

/// How often and how long to wait before retrying a failed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total number of attempts, including the first one.
    pub attempts: u32,
    /// Delay before the first retry; every further retry doubles it.
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    /// Three attempts, waiting about 1 s and then about 2 s.
    fn default() -> Self {
        RetryPolicy {
            attempts: 3,
            base_delay: Duration::from_secs(1),
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries.
    pub const fn none() -> Self {
        RetryPolicy {
            attempts: 1,
            base_delay: Duration::ZERO,
        }
    }

    /// The delay before retry number `retry` (1-based), with ±20% jitter.
    pub fn delay(&self, retry: u32) -> Duration {
        let exponent = retry.saturating_sub(1).min(16);
        let nominal = self.base_delay.saturating_mul(1u32 << exponent);
        nominal.mul_f64(0.8 + 0.4 * random_unit())
    }

    /// Sleeps for [`delay`](Self::delay)`(retry)`.
    pub async fn wait(&self, retry: u32) {
        let delay = self.delay(retry);
        if !delay.is_zero() {
            tracing::debug!(retry, delay_ms = delay.as_millis() as u64, "retrying");
            tokio::time::sleep(delay).await;
        }
    }
}

/// Whether a failed registry call may be repeated: transport failures and `5xx`.
pub fn is_retryable(err: &Error) -> bool {
    match err {
        Error::Http(err) => !err.is_builder() && !err.is_decode(),
        Error::Api { status, .. } => *status >= 500,
        _ => false,
    }
}

/// A uniformly distributed value in `[0, 1)`, good enough for jitter.
fn random_unit() -> f64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(0x5eed);
    (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_double_within_jitter_bounds() {
        let policy = RetryPolicy::default();
        for _ in 0..100 {
            let first = policy.delay(1);
            let second = policy.delay(2);
            assert!(first >= Duration::from_millis(800) && first <= Duration::from_millis(1200));
            assert!(second >= Duration::from_millis(1600) && second <= Duration::from_millis(2400));
        }
    }

    #[test]
    fn none_policy_has_one_attempt_and_no_delay() {
        let policy = RetryPolicy::none();
        assert_eq!(policy.attempts, 1);
        assert_eq!(policy.delay(1), Duration::ZERO);
    }

    #[test]
    fn only_server_errors_are_retryable() {
        let api = |status| Error::Api {
            status,
            code: "x".into(),
            message: "x".into(),
        };
        assert!(is_retryable(&api(500)));
        assert!(is_retryable(&api(503)));
        assert!(!is_retryable(&api(404)));
        assert!(!is_retryable(&Error::VersionExists));
        assert!(!is_retryable(&Error::InvalidInput("x".into())));
    }
}
