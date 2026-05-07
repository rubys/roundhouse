//! Blocking HTTP GET + CGI shell-out wrappers.
//!
//! Two transports for fetching a target response:
//!   - `get`: blocking HTTP GET against a server URL. The standard
//!     path — built once in main, passed through. Status + body
//!     text only; headers/cookies/redirects use reqwest defaults
//!     (follows redirects up to 10 hops, matching browser behavior
//!     for the 303s roundhouse emits on create/update/destroy).
//!   - `shell`: spawn a CGI-shape command per request, parse the
//!     `Status:`-prefixed CGI response from stdout. Used when the
//!     target has no live HTTP server — e.g., the spinel target
//!     after dev_server.rb retirement (see
//!     project_ruby_dev_server_retirement memory). Each invocation
//!     is a fresh process; persistent state goes through whatever
//!     storage the cmd is configured to use (sqlite file etc.).

use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

pub struct Response {
    pub status: u16,
    pub body: String,
}

pub fn get(client: &reqwest::blocking::Client, url: &str) -> Result<Response> {
    let resp = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .with_context(|| format!("read body from {url}"))?;
    Ok(Response { status, body })
}

/// Run `cmd` via `sh -c` with CGI env vars populated for `path`,
/// capture stdout, parse the CGI response. cwd inherited from the
/// caller — scripts/compare cd's into the build dir before invoking
/// the compare tool.
pub fn shell(cmd: &str, path: &str) -> Result<Response> {
    let (path_info, query) = match path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path, ""),
    };

    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .env("REQUEST_METHOD", "GET")
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env("CONTENT_LENGTH", "0")
        .env("CONTENT_TYPE", "")
        .output()
        .with_context(|| format!("spawn target cmd: {cmd}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "target cmd failed for {path} (exit={:?}):\n{stderr}",
            output.status.code()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    parse_cgi(&stdout)
}

/// Parse a CGI-shape response: header lines (CRLF-terminated)
/// including a `Status: <code> <reason>` line, then a blank line,
/// then the body. This matches `runtime/spinel/cgi_io.rb`'s
/// `write_response` shape — see cgi_io.rb:81.
fn parse_cgi(s: &str) -> Result<Response> {
    let sep = s
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow!("no CGI header/body separator in stdout (first 200 chars: {})", &s[..s.len().min(200)]))?;
    let headers = &s[..sep];
    let body = s[sep + 4..].to_string();

    let mut status = 200u16;
    for line in headers.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Status: ") {
            let code_str = rest.split_whitespace().next().unwrap_or(rest);
            status = code_str
                .parse()
                .with_context(|| format!("parse Status code from {rest:?}"))?;
            break;
        }
    }
    Ok(Response { status, body })
}
