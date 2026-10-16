use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// In-memory sliding-window limiter for failed login attempts.
///
/// Keys are recorded per username and per client IP so neither guessing one
/// account from many IPs nor spraying many accounts from one IP gets far.
#[derive(Clone, Debug)]
pub struct LoginThrottle {
    max_attempts: usize,
    window: Duration,
    failures: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl LoginThrottle {
    pub fn new(max_attempts: usize, window: Duration) -> Self {
        Self {
            max_attempts,
            window,
            failures: Arc::default(),
        }
    }

    /// True when any of the keys has reached the failure limit.
    pub fn is_blocked(&self, keys: &[String]) -> bool {
        self.is_blocked_at(keys, Instant::now())
    }

    pub fn record_failure(&self, keys: &[String]) {
        self.record_failure_at(keys, Instant::now());
    }

    pub fn clear(&self, keys: &[String]) {
        let mut failures = self.failures.lock().expect("throttle lock poisoned");
        for key in keys {
            failures.remove(key);
        }
    }

    fn is_blocked_at(&self, keys: &[String], now: Instant) -> bool {
        let mut failures = self.failures.lock().expect("throttle lock poisoned");
        keys.iter().any(|key| {
            failures.get_mut(key).is_some_and(|attempts| {
                attempts.retain(|at| now.duration_since(*at) < self.window);
                attempts.len() >= self.max_attempts
            })
        })
    }

    fn record_failure_at(&self, keys: &[String], now: Instant) {
        let mut failures = self.failures.lock().expect("throttle lock poisoned");
        // Drop idle entries so the map can't grow without bound.
        failures.retain(|_, attempts| {
            attempts.retain(|at| now.duration_since(*at) < self.window);
            !attempts.is_empty()
        });
        for key in keys {
            failures.entry(key.clone()).or_default().push(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::LoginThrottle;

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn blocks_after_the_configured_number_of_failures() {
        let throttle = LoginThrottle::new(3, Duration::from_secs(60));
        let now = Instant::now();
        let alice = keys(&["user:alice", "ip:10.0.0.1"]);

        for _ in 0..2 {
            throttle.record_failure_at(&alice, now);
        }
        assert!(!throttle.is_blocked_at(&alice, now));

        throttle.record_failure_at(&alice, now);
        assert!(throttle.is_blocked_at(&alice, now));
        // The same IP is blocked for other usernames too.
        assert!(throttle.is_blocked_at(&keys(&["user:bob", "ip:10.0.0.1"]), now));
        assert!(!throttle.is_blocked_at(&keys(&["user:bob", "ip:10.0.0.2"]), now));
    }

    #[test]
    fn failures_expire_after_the_window() {
        let throttle = LoginThrottle::new(1, Duration::from_secs(60));
        let start = Instant::now();
        let alice = keys(&["user:alice"]);

        throttle.record_failure_at(&alice, start);
        assert!(throttle.is_blocked_at(&alice, start));
        assert!(!throttle.is_blocked_at(&alice, start + Duration::from_secs(61)));
    }

    #[test]
    fn clear_resets_the_keys() {
        let throttle = LoginThrottle::new(1, Duration::from_secs(60));
        let alice = keys(&["user:alice"]);

        throttle.record_failure(&alice);
        assert!(throttle.is_blocked(&alice));
        throttle.clear(&alice);
        assert!(!throttle.is_blocked(&alice));
    }
}
