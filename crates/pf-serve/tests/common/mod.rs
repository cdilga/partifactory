//! Test helpers: start the real listeners on loopback with an in-memory log capture, and a raw
//! HTTP/1.1 client (so the tests control every header, including spoofed ones).

#![allow(dead_code)]

use pf_obs::testing::Capture;
use pf_serve::{AppState, ServeConfig, ServerBuilder};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const TOKEN: &str = "test-metrics-token";

pub struct Running {
    pub workspace: SocketAddr,
    pub content: SocketAddr,
    pub ops: SocketAddr,
    pub state: AppState,
    pub logs: Capture,
    _guard: tracing::subscriber::DefaultGuard,
    _dir: tempfile::TempDir,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
    }
}

/// Start a server; `f` customises the config and builder. Use `#[tokio::test]` (current-thread)
/// so the log capture sees the server's events.
pub async fn start(
    cfg_fn: impl FnOnce(&mut ServeConfig),
    f: impl FnOnce(&mut ServerBuilder),
) -> Running {
    let logs = Capture::new("info");
    let guard = logs.set_default();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ServeConfig::for_tests(dir.path().to_path_buf(), TOKEN);
    cfg_fn(&mut cfg);
    let mut b = ServerBuilder::new(cfg);
    b.log_handle(logs.handle());
    f(&mut b);
    let bound = b.bind().await.unwrap();
    let (workspace, content, ops) = bound.addrs();
    let state = bound.state();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(bound.run(async move {
        let _ = rx.await;
    }));
    Running {
        workspace,
        content,
        ops,
        state,
        logs,
        _guard: guard,
        _dir: dir,
        stop: Some(tx),
    }
}

#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One request over a fresh connection. `headers` are sent as given (add `Host`).
pub async fn req(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut out = format!(
        "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        out.push_str("Host: parts.pf.test\r\n");
    }
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    s.write_all(out.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Resp {
        status,
        headers,
        body: body.to_string(),
    }
}

pub async fn get(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Resp {
    req(addr, "GET", path, headers, "").await
}

/// Let the access line (logged after the response is written) land.
pub async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}
