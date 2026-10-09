//! An emitted Spinel tree built with `spin build` and driven over HTTP,
//! for a native test whose behavior lives in the server's dispatch
//! (status codes, the request body) rather than in a library call.
//! Under Spinel `main.rb` always starts the server, so a contract
//! script cannot reach the dispatcher; a request has to.
//!
//! Requests carry the session cookie and CSRF token a page handed out,
//! as a browser's would: the ruby family enforces forgery protection on
//! `ActionController::Base` controllers, as Rails does.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// `spin build blog` in `tree`; panics with the compiler's output.
pub fn build(tree: &Path) {
    let output = Command::new("spin")
        .args(["build", "blog"])
        .current_dir(tree)
        .output()
        .unwrap_or_else(|e| panic!("spawn spin: {e}"));
    assert!(
        output.status.success() && tree.join("build/bin/blog").is_file(),
        "`spin build blog` failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        tree.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The built app serving on a free local port with one OS worker and a
/// fresh SQLite file in the tree. Killed when dropped.
pub struct Server {
    child: Child,
    port: u16,
    log: PathBuf,
    cookie: String,
    token: String,
}

pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Server {
    pub fn start(tree: &Path) -> Server {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("pick a free port")
            .port();
        let log = tree.join("server.log");
        let out = std::fs::File::create(&log).expect("create server.log");
        let err = out.try_clone().expect("clone server.log");
        let child = Command::new(tree.join("build/bin/blog"))
            .current_dir(tree)
            .env("PORT", port.to_string())
            .env("BLOG_DB", tree.join("native_http.sqlite3"))
            .env("SPINEL_WORKERS", "1")
            .stdout(out)
            .stderr(err)
            .spawn()
            .expect("start build/bin/blog");
        let mut server = Server { child, port, log, cookie: String::new(), token: String::new() };
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            if let Some(status) = server.child.try_wait().expect("poll the server") {
                panic!("the server exited ({status}) before listening:\n{}", server.log());
            }
            assert!(Instant::now() < deadline, "no listener on {port} within 30s:\n{}", server.log());
            std::thread::sleep(Duration::from_millis(100));
        }
        server
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// GET `path` and keep the session cookie and the page's
    /// `csrf-token` meta for the requests that follow.
    pub fn take_session(&mut self, path: &str) {
        let (status, set_cookies, body) = self.send("GET", path, &[], "");
        assert_eq!(status, 200, "GET {path}:\n{body}\n{}", self.log());
        self.cookie = set_cookies
            .iter()
            .filter_map(|c| c.split(';').next())
            .collect::<Vec<_>>()
            .join("; ");
        let marker = "name=\"csrf-token\" content=\"";
        let at = body.find(marker).unwrap_or_else(|| panic!("no csrf-token meta on {path}:\n{body}"));
        let rest = &body[at + marker.len()..];
        self.token = rest[..rest.find('"').expect("unterminated csrf-token")].to_string();
    }

    /// A POST with the session's cookie and token.
    pub fn post(&self, path: &str, content_type: &str, body: &str) -> Response {
        let headers = [
            ("Content-Type", content_type),
            ("Accept", "application/json"),
            ("Cookie", self.cookie.as_str()),
            ("X-CSRF-Token", self.token.as_str()),
        ];
        let (status, _, body) = self.send("POST", path, &headers, body);
        Response { status, body }
    }

    /// A POST with neither cookie nor token.
    pub fn post_without_session(&self, path: &str, content_type: &str, body: &str) -> Response {
        let headers = [("Content-Type", content_type), ("Accept", "application/json")];
        let (status, _, body) = self.send("POST", path, &headers, body);
        Response { status, body }
    }

    /// One HTTP/1.1 request on its own connection: the status, every
    /// `Set-Cookie` value, and the body.
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, Vec<String>, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        stream.set_read_timeout(Some(Duration::from_secs(30))).expect("read timeout");
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        for (name, value) in headers {
            if !value.is_empty() {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
        }
        request.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
        stream.write_all(request.as_bytes()).expect("send the request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .unwrap_or_else(|e| panic!("{method} {path}: {e}\n{}", self.log()));
        let response = String::from_utf8_lossy(&response).into_owned();
        let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{method} {path}: no status line in {response:?}"));
        let set_cookies = head
            .lines()
            .filter_map(|l| l.split_once(':'))
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value.trim().to_string())
            .collect();
        (status, set_cookies, body.to_string())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
