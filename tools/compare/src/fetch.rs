//! Blocking HTTP GET wrapper.
//!
//! Kept trivial on purpose — the blocking client is built once in
//! main, passed through. We only care about status + body text;
//! headers, cookies, redirects are all server-defaults today
//! (reqwest follows redirects up to 10 hops by default, which
//! matches browser behavior for the 303s roundhouse emits on
//! create/update/destroy).

use anyhow::{Context, Result};

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
