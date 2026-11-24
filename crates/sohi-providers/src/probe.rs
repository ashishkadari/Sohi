//! Finding out whether the online services can be reached (spec §5.3).

use std::time::Duration;

use sohi_core::secret::Secret;

use crate::http::{Auth, HttpClient};
use crate::{ProviderError, mistral};

/// A probe gets this long.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// Whether the API at `base_url` (normally [`mistral::BASE_URL`]) answers.
/// Any HTTP answer counts, whatever its status: a rejected key or a server
/// error is not a network problem.
pub async fn probe(http: &HttpClient, base_url: &str, key: Option<&Secret>) -> bool {
    let url = format!("{base_url}{}", mistral::MODELS_PATH);
    let auth = key.map_or(Auth::None, Auth::Bearer);
    !matches!(
        http.get(&url, auth, TIMEOUT).await,
        Err(ProviderError::Offline(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, closed_port_url, serve_once};

    #[tokio::test]
    async fn any_answer_means_reachable() {
        let http = HttpClient::new().unwrap();
        for status in [200, 401, 403, 404, 429, 500, 503] {
            let server = serve_once(Reply::status(status, "{}")).await;
            assert!(probe(&http, &server.url(""), None).await, "{status}");
            let request = server.request().await;
            assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
            assert!(!request.contains("authorization"));
        }
    }

    #[tokio::test]
    async fn a_refused_connection_does_not() {
        let http = HttpClient::new().unwrap();
        let url = closed_port_url().await;
        let key = Secret::from_input("sk-x").unwrap();
        assert!(!probe(&http, &url, Some(&key)).await);
    }
}
