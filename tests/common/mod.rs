//! Shared helpers: fixture loading and a tiny loopback HTTP server so the
//! async request paths are exercised without touching the internet.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A route: if the request line (method + path?query, lower-cased) contains
/// every needle, respond with `status`, `content_type`, `body`.
#[derive(Clone)]
pub struct Route {
    pub needles: Vec<String>,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

pub fn route(needles: &[&str], content_type: &str, body: Vec<u8>) -> Route {
    Route {
        needles: needles.iter().map(|s| s.to_ascii_lowercase()).collect(),
        status: 200,
        content_type: content_type.to_string(),
        body,
    }
}

/// A recorded request: request line, headers and body.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub line: String,
    pub headers: String,
    pub body: String,
}

pub struct MockServer {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
}

impl MockServer {
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

/// Starts a server on 127.0.0.1 serving `routes`. `{BASE}` in bodies is
/// replaced with the server's base URL, so capabilities can point back here.
pub async fn serve(routes: Vec<Route>) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let routes: Vec<Route> = routes
        .into_iter()
        .map(|mut r| {
            if let Ok(s) = std::str::from_utf8(&r.body) {
                r.body = s.replace("{BASE}", &base).into_bytes();
            }
            r
        })
        .collect();
    let rec = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let routes = routes.clone();
            let rec = rec.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                // Read headers, then the body according to Content-Length.
                let header_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let len = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while buf.len() < header_end + len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                let line = head.lines().next().unwrap_or_default().to_string();
                let decoded = percent_decode(&line).to_ascii_lowercase();
                rec.lock().unwrap().push(Recorded {
                    line: percent_decode(&line),
                    headers: head.clone(),
                    body: body.clone(),
                });
                let hay = format!("{decoded} {}", body.to_ascii_lowercase());
                let found = routes
                    .iter()
                    .find(|r| r.needles.iter().all(|n| hay.contains(n.as_str())));
                let (status, ct, payload) = match found {
                    Some(r) => (r.status, r.content_type.clone(), r.body.clone()),
                    None => (404, "text/plain".into(), b"no route".to_vec()),
                };
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(&payload).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    MockServer { base, requests }
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let h = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz");
                match u8::from_str_radix(h, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
