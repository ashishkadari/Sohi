//! The shared HTTP client (spec §2.2, §5.5, §9.2).
//!
//! TLS is Windows' own `SChannel` via `native-tls`, so no crypto library is
//! bundled. One client is shared by every provider so connections are
//! pooled; idle ones close after 90 s.

use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderValue};
use sohi_core::secret::Secret;

use crate::ProviderError;

/// How long connecting (TCP + TLS) may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Idle pooled connections are closed after this long.
pub const POOL_IDLE: Duration = Duration::from_secs(90);
/// How much of an error body is kept for the log.
const ERROR_BODY_LIMIT: usize = 512;

/// How a request proves who it is.
#[derive(Debug, Clone, Copy)]
pub enum Auth<'a> {
    /// No credentials (a local service such as Ollama).
    None,
    /// `Authorization: Bearer <key>` (Mistral).
    Bearer(&'a Secret),
    /// The key in a named header (Anthropic's `x-api-key`).
    Header(&'static str, &'a Secret),
}

/// The process-wide HTTP client.
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
}

impl HttpClient {
    /// Builds the client.
    ///
    /// # Errors
    /// Returns [`ProviderError::Offline`] if the TLS backend cannot start.
    pub fn new() -> Result<Self, ProviderError> {
        let inner = reqwest::Client::builder()
            .tls_backend_native()
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_idle_timeout(POOL_IDLE)
            .user_agent(concat!("Sohi/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::Offline(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Sends a JSON `body` to `url` and waits for the response headers, at
    /// most `first_byte` long. A non-success status becomes an error.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] for transport failures, timeouts and
    /// non-success statuses.
    pub async fn post_json(
        &self,
        url: &str,
        auth: Auth<'_>,
        headers: &[(&'static str, &'static str)],
        body: Vec<u8>,
        first_byte: Duration,
    ) -> Result<Response, ProviderError> {
        self.post(url, auth, "application/json", headers, body, first_byte)
            .await
    }

    /// Like [`Self::post_json`] for a body of any `content_type`.
    ///
    /// # Errors
    /// Returns a [`ProviderError`] like [`Self::post_json`].
    pub async fn post(
        &self,
        url: &str,
        auth: Auth<'_>,
        content_type: &str,
        headers: &[(&'static str, &'static str)],
        body: Vec<u8>,
        first_byte: Duration,
    ) -> Result<Response, ProviderError> {
        let mut request = self
            .inner
            .post(url)
            .header(CONTENT_TYPE, content_type)
            .body(body);
        for &(name, value) in headers {
            request = request.header(name, value);
        }
        request = match auth {
            Auth::None => request,
            Auth::Bearer(key) => request.bearer_auth(key.expose()),
            Auth::Header(name, key) => request.header(name, sensitive(key)?),
        };
        let again = request.try_clone();
        let mut sent = send(request, first_byte).await?;
        // A pooled connection can be closed by the server just as it is
        // used. Nothing came back, so the request is sent once more.
        if let Some(again) = again
            && sent.as_ref().is_err_and(was_dropped)
        {
            sent = send(again, first_byte).await?;
        }
        let response = sent.map_err(|e| ProviderError::from_transport(&e))?;
        Response::check(response).await
    }

    /// Fetches `url` with a short timeout and reports only success or
    /// failure (connectivity probes and prewarming).
    ///
    /// # Errors
    /// Returns a [`ProviderError`] like [`Self::post_json`].
    pub async fn get(
        &self,
        url: &str,
        auth: Auth<'_>,
        timeout: Duration,
    ) -> Result<Response, ProviderError> {
        let request = self.inner.get(url);
        let request = match auth {
            Auth::None => request,
            Auth::Bearer(key) => request.bearer_auth(key.expose()),
            Auth::Header(name, key) => request.header(name, sensitive(key)?),
        };
        let response = tokio::time::timeout(timeout, request.send())
            .await
            .map_err(|_| ProviderError::Offline("no response in time".to_owned()))?
            .map_err(|e| ProviderError::from_transport(&e))?;
        Response::check(response).await
    }
}

/// Sends `request` and waits for the response headers, at most `limit`
/// long.
async fn send(
    request: reqwest::RequestBuilder,
    limit: Duration,
) -> Result<reqwest::Result<reqwest::Response>, ProviderError> {
    tokio::time::timeout(limit, request.send())
        .await
        .map_err(|_| ProviderError::Offline("no response in time".to_owned()))
}

/// Whether a connection that stood broke before anything came back.
fn was_dropped(error: &reqwest::Error) -> bool {
    error.is_request() && !error.is_connect() && !error.is_timeout()
}

/// A header value holding a key, marked sensitive so it is never shown in
/// debug output.
fn sensitive(key: &Secret) -> Result<HeaderValue, ProviderError> {
    let mut value = HeaderValue::from_str(key.expose())
        .map_err(|_| ProviderError::BadResponse("the stored key has invalid characters".into()))?;
    value.set_sensitive(true);
    Ok(value)
}

/// A successful response whose body is read in chunks.
#[derive(Debug)]
pub struct Response {
    inner: reqwest::Response,
}

impl Response {
    async fn check(response: reqwest::Response) -> Result<Self, ProviderError> {
        let status = response.status();
        if status.is_success() {
            return Ok(Self { inner: response });
        }
        // The body usually says why; keep its start for the log.
        let body = response.text().await.unwrap_or_default();
        let detail: String = body.chars().take(ERROR_BODY_LIMIT).collect();
        Err(ProviderError::from_status(status.as_u16(), &detail))
    }

    /// The next piece of the body, or `None` at the end. Waits at most
    /// `timeout` for it.
    ///
    /// # Errors
    /// Returns [`ProviderError::Offline`] if the connection drops or
    /// stalls.
    pub async fn next_chunk(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<impl AsRef<[u8]> + use<>>, ProviderError> {
        tokio::time::timeout(timeout, self.inner.chunk())
            .await
            .map_err(|_| ProviderError::Offline("the response stalled".to_owned()))?
            .map_err(|e| ProviderError::from_transport(&e))
    }

    /// The whole remaining body.
    ///
    /// # Errors
    /// Returns [`ProviderError::Offline`] if the connection drops.
    pub async fn bytes(self) -> Result<Vec<u8>, ProviderError> {
        self.inner
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| ProviderError::from_transport(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testserver::{Reply, serve_once};

    /// Drops the first `drops` connections once a request arrives, then
    /// answers. Returns its address.
    async fn flaky(drops: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            for n in 0..=drops {
                let (mut socket, _) = listener.accept().await.unwrap();
                let _ = socket.read(&mut [0; 1024]).await;
                if n >= drops {
                    let ok = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
                    let _ = socket.write_all(ok).await;
                }
            }
        });
        url
    }

    async fn post(url: &str) -> Result<Response, ProviderError> {
        let wait = Duration::from_secs(5);
        HttpClient::new()
            .unwrap()
            .post_json(url, Auth::None, &[], b"{}".to_vec(), wait)
            .await
    }

    #[tokio::test]
    async fn a_dropped_connection_is_tried_once_more() {
        let response = post(&flaky(1).await).await.unwrap();
        assert_eq!(response.bytes().await.unwrap(), b"ok");
        let error = post(&flaky(2).await).await.unwrap_err();
        assert!(matches!(error, ProviderError::Offline(_)));
    }

    const SECOND: Duration = Duration::from_secs(1);

    fn key(text: &str) -> Secret {
        Secret::from_input(text).unwrap()
    }

    #[tokio::test]
    async fn posts_json_with_bearer_auth_and_streams_the_body() {
        let server = serve_once(Reply::chunked(200, &["first ", "second"])).await;
        let client = HttpClient::new().unwrap();
        let key = key("sk-test");
        let mut response = client
            .post_json(
                &server.url("/v1/chat"),
                Auth::Bearer(&key),
                &[("x-extra", "1")],
                b"{\"a\":1}".to_vec(),
                SECOND,
            )
            .await
            .unwrap();
        let mut body = Vec::new();
        while let Some(chunk) = response.next_chunk(SECOND).await.unwrap() {
            body.extend_from_slice(chunk.as_ref());
        }
        assert_eq!(body, b"first second");
        let request = server.request().await;
        assert!(
            request.starts_with("POST /v1/chat HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("authorization: Bearer sk-test\r\n"),
            "{request}"
        );
        assert!(request.contains("x-extra: 1\r\n"));
        assert!(request.contains("content-type: application/json\r\n"));
        assert!(request.ends_with("\r\n\r\n{\"a\":1}"));
    }

    #[tokio::test]
    async fn header_auth_puts_the_key_in_that_header() {
        let server = serve_once(Reply::status(200, "ok")).await;
        let client = HttpClient::new().unwrap();
        let key = key("anthropic-key");
        let auth = Auth::Header("x-api-key", &key);
        client.get(&server.url("/"), auth, SECOND).await.unwrap();
        assert!(
            server
                .request()
                .await
                .contains("x-api-key: anthropic-key\r\n")
        );
    }

    #[tokio::test]
    async fn error_statuses_are_classified() {
        for (status, want) in [
            (401, ProviderError::Auth(401)),
            (429, ProviderError::RateLimited),
            (503, ProviderError::Server(503)),
        ] {
            let server = serve_once(Reply::status(status, "nope")).await;
            let client = HttpClient::new().unwrap();
            let got = client
                .post_json(&server.url("/"), Auth::None, &[], Vec::new(), SECOND)
                .await
                .unwrap_err();
            assert_eq!(got, want);
        }
    }

    #[tokio::test]
    async fn a_closed_port_is_offline() {
        let url = crate::testserver::closed_port_url().await;
        let client = HttpClient::new().unwrap();
        let got = client.get(&url, Auth::None, SECOND).await.unwrap_err();
        assert!(matches!(got, ProviderError::Offline(_)), "{got:?}");
    }

    #[tokio::test]
    async fn a_stalled_body_times_out_as_offline() {
        let server = serve_once(Reply::stalled(200, "partial")).await;
        let client = HttpClient::new().unwrap();
        let mut response = client
            .get(&server.url("/"), Auth::None, SECOND)
            .await
            .unwrap();
        let short = Duration::from_millis(150);
        assert!(response.next_chunk(short).await.unwrap().is_some());
        let got = response.next_chunk(short).await.err().unwrap();
        assert!(matches!(got, ProviderError::Offline(_)), "{got:?}");
    }

    /// Live TLS check against the real Mistral endpoint. Without a key the
    /// service answers 401, which proves DNS, TCP and `SChannel` all work.
    #[tokio::test]
    #[ignore = "needs the internet"]
    async fn schannel_reaches_mistral() {
        let client = HttpClient::new().unwrap();
        let got = client
            .get("https://api.mistral.ai/v1/models", Auth::None, 10 * SECOND)
            .await
            .unwrap_err();
        assert_eq!(got, ProviderError::Auth(401));
    }
}
