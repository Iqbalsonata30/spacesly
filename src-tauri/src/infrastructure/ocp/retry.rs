use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::errors::{OcpError, OcpErrorKind, OcpResult};

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(4),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    pub fn none() -> Self {
        Self {
            attempts: 1,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            jitter: false,
        }
    }

    fn delay_for(&self, attempt: u32) -> Duration {
        if self.attempts <= 1 || self.base_delay.is_zero() {
            return Duration::ZERO;
        }
        let exponent = attempt.saturating_sub(1).min(6);
        let multiplier = 1u32 << exponent;
        let mut delay = self.base_delay.saturating_mul(multiplier);
        if delay > self.max_delay {
            delay = self.max_delay;
        }
        if self.jitter && !delay.is_zero() {
            let millis = delay.as_millis() as u64;
            let mixed = millis
                .wrapping_mul(0x9E3779B97F4A7C15)
                .wrapping_add(attempt as u64);
            let jitter = (mixed % 25).min(millis);
            delay = Duration::from_millis(jitter);
        }
        delay
    }
}

pub fn with_retry<T>(
    policy: RetryPolicy,
    cancelled: &AtomicBool,
    mut operation: impl FnMut() -> OcpResult<T>,
) -> OcpResult<T> {
    let attempts = policy.attempts.max(1);
    let mut last_error = OcpError::internal("retry operation never ran");
    for attempt in 1..=attempts {
        if cancelled.load(Ordering::Relaxed) {
            return Err(OcpError::cancelled("OCP operation was cancelled."));
        }
        let started = Instant::now();
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = error;
                if !last_error.is_retryable() || attempt == attempts {
                    return Err(last_error);
                }
            }
        }
        let elapsed = started.elapsed();
        let delay = policy.delay_for(attempt);
        let remaining = delay.saturating_sub(elapsed);
        if remaining.is_zero() {
            continue;
        }
        let deadline = Instant::now() + remaining;
        while Instant::now() < deadline {
            if cancelled.load(Ordering::Relaxed) {
                return Err(OcpError::cancelled("OCP retry was cancelled."));
            }
            let slice = remaining.min(Duration::from_millis(50));
            if !slice.is_zero() {
                thread::sleep(slice);
            }
        }
    }
    match last_error.kind {
        OcpErrorKind::Cancelled => Err(last_error),
        _ => Err(last_error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn retries_transient_failures_up_to_policy_attempts() {
        let attempts = AtomicU32::new(0);
        let policy = RetryPolicy {
            attempts: 4,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            jitter: false,
        };
        let result = with_retry(policy, &AtomicBool::new(false), || {
            let count = attempts.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(OcpError::connect("transient", "boom"))
            } else {
                Ok(count)
            }
        });
        assert_eq!(result.unwrap(), 2);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn permanent_errors_abort_immediately() {
        let attempts = AtomicU32::new(0);
        let result: OcpResult<u32> =
            with_retry(RetryPolicy::default(), &AtomicBool::new(false), || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(OcpError::auth("rejected", "unauthorized"))
            });
        assert!(matches!(result, Err(ref error) if error.kind == OcpErrorKind::Auth));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancellation_interrupts_retry_loop() {
        let cancelled = AtomicBool::new(true);
        let result: OcpResult<u32> = with_retry(RetryPolicy::default(), &cancelled, || {
            Err(OcpError::connect("transient", "boom"))
        });
        assert!(result.unwrap_err().is_cancelled());
    }

    #[test]
    fn returns_last_transient_error_when_attempts_exhausted() {
        let attempts = AtomicU32::new(0);
        let policy = RetryPolicy {
            attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            jitter: false,
        };
        let result: OcpResult<u32> = with_retry(policy, &AtomicBool::new(false), || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(OcpError::timeout("slow", "timed out"))
        });
        assert!(matches!(result, Err(ref error) if error.kind == OcpErrorKind::Timeout));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn backoff_delay_grows_exponentially_then_caps() {
        let policy = RetryPolicy {
            attempts: 10,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(100),
            jitter: false,
        };
        assert_eq!(policy.delay_for(1), Duration::from_millis(10));
        assert_eq!(policy.delay_for(2), Duration::from_millis(20));
        assert_eq!(policy.delay_for(4), Duration::from_millis(80));
        assert_eq!(policy.delay_for(5), Duration::from_millis(100));
        assert_eq!(policy.delay_for(9), Duration::from_millis(100));
    }
}
