//! A one-shot local HTTP/1.1 server for tests (spec §9.6: a mock provider
//! server, so tests are deterministic and offline).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// What the server sends back.
pub struct Reply {
    status: u16,
    /// Body pieces, sent as separate chunks with a short pause between.
    chunks: Vec<String>,
    /// Keep the connection open without finishing the body.
    stall: bool,
}

impl Reply {
    /// A plain response with `body`.
    pub fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            chunks: vec![body.to_owned()],
            stall: false,
        }
    }

    /// A chunked response, one HTTP chunk per piece.
    pub fn chunked(status: u16, pieces: &[&str]) -> Self {
        Self {
            status,
            chunks: pieces.iter().map(|&p| p.to_owned()).collect(),
            stall: false,
        }
    }

    /// Sends `first` as a chunk, then never finishes.
    pub fn stalled(status: u16, first: &str) -> Self {
        Self {
            stall: true,
            ..Self::chunked(status, &[first])
        }
    }
}

/// A running server that answers one request.
pub struct Server {
    port: u16,
    request: oneshot::Receiver<String>,
}

impl Server {
    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// The raw request the server received (headers lowercased by the
    /// client, then the body).
    pub async fn request(self) -> String {
        self.request.await.unwrap_or_default()
    }
}

/// Starts a server on a free port that answers one request with `reply`.
pub async fn serve_once(reply: Reply) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let _ = tx.send(request);
        let head = format!(
            "HTTP/1.1 {} X\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            reply.status
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        for chunk in &reply.chunks {
            let frame = format!("{:x}\r\n{chunk}\r\n", chunk.len());
            if socket.write_all(frame.as_bytes()).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if reply.stall {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    Server { port, request: rx }
}

/// Reads the request head and a `content-length` body.
async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = socket.read(&mut buf).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

/// A URL on a port nothing listens on.
pub async fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}/")
}
