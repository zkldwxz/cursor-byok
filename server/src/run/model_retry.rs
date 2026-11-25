//! Defines retry policy for one logical model call.

use std::time::Duration;

use super::ModelCycleFailure;

/// Attempts after the first one. Rate-limited gateways recover on the order of
/// a minute, so eight attempts over an exponential ladder is the longest wait
/// that still feels like a retry rather than a hang.
pub(super) const MAX_MODEL_RETRIES: u32 = 8;
pub(super) const MODEL_RETRY_BASE_DELAY: Duration = Duration::from_secs(2);
pub(super) const MODEL_RETRY_MAX_DELAY: Duration = Duration::from_secs(60);

pub(super) fn should_retry(failure: &ModelCycleFailure, retries: u32) -> bool {
    failure.retryable && retries < MAX_MODEL_RETRIES
}

/// Delay before attempt `retries + 1`.
///
/// Exponential with jitter, because parallel runs hit the same rate limit at the
/// same moment: a fixed delay keeps them in lockstep and every retry round
/// collides again. Jitter spreads them out without any coordination.
pub(super) fn retry_delay(failure: &ModelCycleFailure, retries: u32) -> Duration {
    let exponential = MODEL_RETRY_BASE_DELAY
        .saturating_mul(1_u32 << retries.min(16))
        .min(MODEL_RETRY_MAX_DELAY);
    let ceiling_ms = exponential.as_millis().min(u128::from(u32::MAX)) as u32;
    let jittered = if ceiling_ms == 0 {
        0
    } else {
        // Half the ceiling fixed, half random: the floor keeps the ladder
        // monotonic, the random half breaks the lockstep.
        let half = ceiling_ms / 2;
        half + pseudo_random(retries, failure) % (ceiling_ms - half + 1)
    };
    Duration::from_millis(u64::from(jittered))
}

/// Deterministic per-attempt spread; the run id is not available here and a
/// real RNG is not worth a dependency for a retry delay.
fn pseudo_random(retries: u32, failure: &ModelCycleFailure) -> u32 {
    let mut hash = 2_166_136_261_u32 ^ retries;
    for byte in failure.failure.message().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{ModelCycleFailure, RunFailure};

    fn failure(retryable: bool) -> ModelCycleFailure {
        ModelCycleFailure {
            failure: RunFailure::Provider("failed".into()),
            partial_text: String::new(),
            partial_reasoning: String::new(),
            usage: None,
            retryable,
        }
    }

    #[test]
    fn permits_eight_retries_after_the_initial_attempt() {
        let retryable = failure(true);
        for retries in 0..MAX_MODEL_RETRIES {
            assert!(should_retry(&retryable, retries));
        }
        assert!(!should_retry(&retryable, MAX_MODEL_RETRIES));
    }

    #[test]
    fn terminal_failures_never_retry() {
        assert!(!should_retry(&failure(false), 0));
    }

    #[test]
    fn the_delay_grows_and_stays_within_the_ceiling() {
        let failed = failure(true);
        let mut previous = Duration::ZERO;
        for retries in 0..MAX_MODEL_RETRIES {
            let delay = retry_delay(&failed, retries);
            assert!(
                delay >= previous,
                "retry {retries} waited {delay:?}, less than {previous:?}"
            );
            assert!(delay <= MODEL_RETRY_MAX_DELAY, "{delay:?}");
            previous = delay;
        }
        assert!(retry_delay(&failed, 0) >= MODEL_RETRY_BASE_DELAY / 2);
    }

    #[test]
    fn parallel_failures_do_not_share_one_delay() {
        let first = ModelCycleFailure {
            failure: RunFailure::Provider("OpenAI Chat 429 Too Many Requests".into()),
            partial_text: String::new(),
            partial_reasoning: String::new(),
            usage: None,
            retryable: true,
        };
        let second = ModelCycleFailure {
            failure: RunFailure::Provider("Anthropic 529 Overloaded".into()),
            partial_text: String::new(),
            partial_reasoning: String::new(),
            usage: None,
            retryable: true,
        };
        assert_ne!(retry_delay(&first, 3), retry_delay(&second, 3));
    }
}
