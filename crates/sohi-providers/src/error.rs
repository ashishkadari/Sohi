//! Provider failures, classified the way the state machine needs them
//! (spec §9.2). Only [`ProviderError::Offline`] says anything about
//! connectivity; every other failure happened on a working network.

/// Why a provider request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// No API key is stored for the provider.
    #[error("no API key is stored for {0}")]
    MissingKey(&'static str),
    /// DNS, connect, TLS or a timeout: the service could not be reached.
    #[error("cannot reach the service: {0}")]
    Offline(String),
    /// HTTP 401 or 403: the key is missing, wrong or revoked.
    #[error("the service rejected the API key (HTTP {0})")]
    Auth(u16),
    /// HTTP 429.
    #[error("rate limited (HTTP 429)")]
    RateLimited,
    /// HTTP 5xx.
    #[error("server error (HTTP {0})")]
    Server(u16),
    /// Any other unexpected status, or a body Sohi could not understand.
    #[error("unexpected response: {0}")]
    BadResponse(String),
    /// The turn was cancelled.
    #[error("cancelled")]
    Cancelled,
}

impl ProviderError {
    /// Classifies a non-success HTTP status. `detail` is the start of the
    /// response body, kept for the log.
    #[must_use]
    pub fn from_status(status: u16, detail: &str) -> Self {
        match status {
            401 | 403 => Self::Auth(status),
            429 => Self::RateLimited,
            500..=599 => Self::Server(status),
            _ => Self::BadResponse(format!("HTTP {status}: {detail}")),
        }
    }

    /// Classifies a transport error from the HTTP client.
    #[must_use]
    pub fn from_transport(err: &reqwest::Error) -> Self {
        if let Some(status) = err.status() {
            return Self::from_status(status.as_u16(), "");
        }
        if err.is_decode() {
            return Self::BadResponse(err.to_string());
        }
        // Connect, TLS, timeout, or the connection dropped mid-body: in
        // every case the service could not be reached reliably.
        Self::Offline(err.to_string())
    }

    /// Whether repeating the same request might succeed (spec §5.3: one
    /// retry for 429 and 5xx).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Server(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_the_spec_classes() {
        assert_eq!(
            ProviderError::from_status(401, ""),
            ProviderError::Auth(401)
        );
        assert_eq!(
            ProviderError::from_status(403, ""),
            ProviderError::Auth(403)
        );
        assert_eq!(
            ProviderError::from_status(429, ""),
            ProviderError::RateLimited
        );
        assert_eq!(
            ProviderError::from_status(503, ""),
            ProviderError::Server(503)
        );
        assert_eq!(
            ProviderError::from_status(400, "bad model"),
            ProviderError::BadResponse("HTTP 400: bad model".to_owned())
        );
    }

    #[test]
    fn only_rate_limits_and_server_errors_retry() {
        assert!(ProviderError::RateLimited.is_retryable());
        assert!(ProviderError::Server(502).is_retryable());
        assert!(!ProviderError::Auth(401).is_retryable());
        assert!(!ProviderError::Offline(String::new()).is_retryable());
        assert!(!ProviderError::Cancelled.is_retryable());
    }
}
