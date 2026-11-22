//! The retry policy (spec §5.3, §9.2): a 429 or 5xx gets one more try
//! after about 300 ms, but only while nothing has been shown yet. A reply
//! already under way is never repeated.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ProviderError;

/// The pause before the retry, before jitter.
pub const RETRY_DELAY: Duration = Duration::from_millis(300);

/// [`RETRY_DELAY`] jittered by up to ±50 % so clients that failed together
/// do not retry together.
#[must_use]
pub fn retry_delay(seed: u32) -> Duration {
    let half = RETRY_DELAY / 2;
    let spread = u32::try_from(RETRY_DELAY.as_millis()).unwrap_or(u32::MAX);
    half + Duration::from_millis(u64::from(seed % (spread + 1)))
}

/// A seed for [`retry_delay`]: the clock's nanoseconds. No randomness
/// crate is needed for this.
#[must_use]
pub fn jitter_seed() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos())
}

/// Runs `attempt`, and once more after `delay` if it failed with a
/// retryable error before writing anything to `reply`. `on_retry` is told
/// why, for the log.
///
/// # Errors
/// Returns the last attempt's error.
pub async fn retry_once(
    reply: &mut String,
    delay: Duration,
    on_retry: impl FnOnce(&ProviderError),
    mut attempt: impl AsyncFnMut(&mut String) -> Result<(), ProviderError>,
) -> Result<(), ProviderError> {
    match attempt(reply).await {
        Err(error) if error.is_retryable() && reply.is_empty() => {
            on_retry(&error);
            tokio::time::sleep(delay).await;
            attempt(reply).await
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delay_stays_within_half_and_one_and_a_half() {
        for seed in [0, 1, 150, 300, 301, u32::MAX] {
            let delay = retry_delay(seed);
            assert!(delay >= Duration::from_millis(150), "{delay:?}");
            assert!(delay <= Duration::from_millis(450), "{delay:?}");
        }
        assert_ne!(retry_delay(0), retry_delay(200));
    }

    /// Runs `retry_once` over scripted outcomes; each outcome may first
    /// write some text.
    async fn script(
        outcomes: &[(&str, Result<(), ProviderError>)],
    ) -> (Result<(), ProviderError>, usize, bool) {
        let mut reply = String::new();
        let mut calls = 0;
        let mut retried = false;
        let result = retry_once(
            &mut reply,
            Duration::ZERO,
            |_| retried = true,
            async |reply: &mut String| {
                let (text, outcome) = &outcomes[calls];
                calls += 1;
                reply.push_str(text);
                outcome.clone()
            },
        )
        .await;
        (result, calls, retried)
    }

    #[tokio::test]
    async fn a_rate_limit_is_retried_once() {
        let (result, calls, retried) =
            script(&[("", Err(ProviderError::RateLimited)), ("ok", Ok(()))]).await;
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 2);
        assert!(retried);
    }

    #[tokio::test]
    async fn only_once() {
        let (result, calls, _) = script(&[
            ("", Err(ProviderError::Server(503))),
            ("", Err(ProviderError::Server(502))),
        ])
        .await;
        assert_eq!(result, Err(ProviderError::Server(502)));
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn other_errors_are_not_retried() {
        for error in [
            ProviderError::Auth(401),
            ProviderError::Offline(String::new()),
            ProviderError::Refused,
        ] {
            let (result, calls, retried) = script(&[("", Err(error.clone()))]).await;
            assert_eq!(result, Err(error));
            assert_eq!((calls, retried), (1, false));
        }
    }

    #[tokio::test]
    async fn a_reply_under_way_is_not_repeated() {
        let (result, calls, _) = script(&[("half", Err(ProviderError::Server(529)))]).await;
        assert_eq!(result, Err(ProviderError::Server(529)));
        assert_eq!(calls, 1);
    }
}
