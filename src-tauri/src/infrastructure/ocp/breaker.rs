use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::errors::{OcpError, OcpResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

impl BreakerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// Serializable, transport-friendly view of a breaker's runtime state so the
/// long-lived MCP subprocess can share its health with the parent process
/// (e.g. via a `breaker.json` file next to `credentials.json`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BreakerSnapshot {
    pub threshold: u32,
    pub reset_after_secs: u64,
    pub consecutive_failures: u32,
    /// Milliseconds remaining in the open window, if any.
    pub open_remaining_ms: Option<u64>,
    pub half_open_trial: bool,
}

pub struct CircuitBreaker {
    threshold: u32,
    reset_after: Duration,
    consecutive_failures: Mutex<u32>,
    open_until: Mutex<Option<Instant>>,
    half_open_trial: Mutex<bool>,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, reset_after: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            reset_after,
            consecutive_failures: Mutex::new(0),
            open_until: Mutex::new(None),
            half_open_trial: Mutex::new(false),
        }
    }

    pub fn state(&self) -> BreakerState {
        let failures = self
            .consecutive_failures
            .lock()
            .map(|guard| *guard)
            .unwrap_or(0);
        let open_until = self.open_until.lock().map(|guard| *guard).unwrap_or(None);
        let half_open = self
            .half_open_trial
            .lock()
            .map(|guard| *guard)
            .unwrap_or(false);
        if half_open {
            return BreakerState::HalfOpen;
        }
        if let Some(until) = open_until {
            if Instant::now() < until {
                return BreakerState::Open;
            }
        }
        if failures >= self.threshold {
            BreakerState::HalfOpen
        } else {
            BreakerState::Closed
        }
    }

    pub fn allow(&self) -> OcpResult<()> {
        match self.state() {
            BreakerState::Closed => Ok(()),
            BreakerState::HalfOpen => {
                if let Ok(mut trial) = self.half_open_trial.try_lock() {
                    if *trial {
                        return Err(self.open_error());
                    }
                    *trial = true;
                    Ok(())
                } else {
                    Err(self.open_error())
                }
            }
            BreakerState::Open => Err(self.open_error()),
        }
    }

    pub fn record_success(&self) {
        let _ = self.consecutive_failures.lock().map(|mut count| {
            *count = 0;
        });
        let _ = self.open_until.lock().map(|mut until| {
            *until = None;
        });
        let _ = self.half_open_trial.lock().map(|mut trial| {
            *trial = false;
        });
    }

    pub fn record_failure(&self) {
        let failures = self
            .consecutive_failures
            .lock()
            .map(|mut guard| {
                *guard += 1;
                *guard
            })
            .unwrap_or(1);
        if failures >= self.threshold {
            let _ = self.open_until.lock().map(|mut until| {
                *until = Some(Instant::now() + self.reset_after);
            });
            let _ = self.half_open_trial.lock().map(|mut trial| {
                *trial = false;
            });
        }
    }

    fn open_error(&self) -> OcpError {
        OcpError::connect(
            "circuit_open",
            "OCP connector circuit is open after repeated failures. Retry after the reset window.",
        )
    }

    /// Capture the current breaker state for persistence/observation. The open
    /// window is reported as a remaining duration (epoch `Instant`s are not
    /// serializable and only meaningful inside the owning process).
    pub fn snapshot(&self) -> BreakerSnapshot {
        BreakerSnapshot {
            threshold: self.threshold,
            reset_after_secs: self.reset_after.as_secs(),
            consecutive_failures: self
                .consecutive_failures
                .lock()
                .map(|guard| *guard)
                .unwrap_or(0),
            open_remaining_ms: self
                .open_until
                .lock()
                .map(|guard| {
                    guard.map(|until| {
                        until
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(u64::MAX as u128) as u64
                    })
                })
                .unwrap_or(None),
            half_open_trial: self
                .half_open_trial
                .lock()
                .map(|guard| *guard)
                .unwrap_or(false),
        }
    }

    /// Reconstruct a breaker from a persisted snapshot. Open windows are
    /// recomputed relative to the current process clock, so a stale snapshot
    /// whose window has already elapsed will naturally report `HalfOpen`.
    pub fn from_snapshot(snapshot: &BreakerSnapshot) -> Self {
        let breaker = Self::new(
            snapshot.threshold,
            Duration::from_secs(snapshot.reset_after_secs),
        );
        let _ = breaker.consecutive_failures.lock().map(|mut count| {
            *count = snapshot.consecutive_failures;
        });
        let _ = breaker.open_until.lock().map(|mut until| {
            *until = snapshot.open_remaining_ms.map(|ms| {
                let now = Instant::now();
                now + Duration::from_millis(ms)
            });
        });
        let _ = breaker.half_open_trial.lock().map(|mut trial| {
            *trial = snapshot.half_open_trial;
        });
        breaker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_threshold_and_then_allows_half_open_probe() {
        let breaker = CircuitBreaker::new(2, Duration::from_secs(60));
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow().is_ok());

        breaker.record_failure();
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);
        assert!(breaker.allow().is_err());

        let mut open_until = breaker.open_until.lock().unwrap();
        *open_until = Some(Instant::now() - Duration::from_secs(1));
        drop(open_until);

        assert_eq!(breaker.state(), BreakerState::HalfOpen);
        assert!(breaker.allow().is_ok());
        assert!(breaker.allow().is_err());
    }

    #[test]
    fn success_on_half_open_probe_closes_the_circuit() {
        let breaker = CircuitBreaker::new(1, Duration::from_secs(60));
        breaker.record_failure();
        assert_eq!(breaker.state(), BreakerState::Open);

        let mut open_until = breaker.open_until.lock().unwrap();
        *open_until = Some(Instant::now() - Duration::from_secs(1));
        drop(open_until);

        breaker.allow().unwrap();
        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow().is_ok());
    }

    #[test]
    fn reset_clears_failure_counts() {
        let breaker = CircuitBreaker::new(3, Duration::from_secs(60));
        breaker.record_failure();
        breaker.record_failure();
        breaker.record_success();
        assert_eq!(breaker.state(), BreakerState::Closed);
        assert!(breaker.allow().is_ok());
    }

    #[test]
    fn snapshot_roundtrips_failure_count_and_half_open_flag() {
        let breaker = CircuitBreaker::new(3, Duration::from_secs(30));
        breaker.record_failure();
        breaker.record_failure();
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot.threshold, 3);
        assert_eq!(snapshot.reset_after_secs, 30);
        assert_eq!(snapshot.consecutive_failures, 2);
        assert!(!snapshot.half_open_trial);

        let restored = CircuitBreaker::from_snapshot(&snapshot);
        assert_eq!(*restored.consecutive_failures.lock().unwrap(), 2);
        assert!(!*restored.half_open_trial.lock().unwrap());
        assert_eq!(restored.state(), BreakerState::Closed);
    }

    #[test]
    fn snapshot_reports_open_window_and_reconstructs_it() {
        let breaker = CircuitBreaker::new(1, Duration::from_secs(30));
        breaker.record_failure();
        let snapshot = breaker.snapshot();
        assert_eq!(snapshot.consecutive_failures, 1);
        assert!(
            snapshot.open_remaining_ms.unwrap() > 0,
            "open window should still be counting down"
        );

        let restored = CircuitBreaker::from_snapshot(&snapshot);
        assert_eq!(restored.state(), BreakerState::Open);
        assert!(restored.allow().is_err());
    }

    #[test]
    fn stale_open_snapshot_reports_half_open() {
        let snapshot = BreakerSnapshot {
            threshold: 2,
            reset_after_secs: 30,
            consecutive_failures: 2,
            open_remaining_ms: Some(0),
            half_open_trial: false,
        };
        let restored = CircuitBreaker::from_snapshot(&snapshot);
        assert_eq!(restored.state(), BreakerState::HalfOpen);
    }
}
