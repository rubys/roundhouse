//! Roundhouse Rust server runtime.
//!
//! Hand-written, shipped alongside generated code (copied in by the
//! Rust emitter as `src/server.rs`). The emitted `main.rs` calls
//! `start(router, opts)` to open the production DB, apply
//! schema, install middleware, and run axum.
//!
//! Middleware stack (outer → inner):
//!   - `layout_wrap` — wraps HTML responses in the full document
//!     shell (Tailwind + importmap + Action Cable meta). Mirrors
//!     the TS runtime's `renderLayout`.
//!   - `method_override` — Rails forms POST `_method=patch|
//!     put|delete`; we read the form body, rewrite the request
//!     method, and re-inject the body so downstream `axum::Form`
//!     extractors still work.
//!
//! Action Cable / WebSocket support stubs today — the
//! `turbo_stream_from` helper emits a valid subscription tag, but
//! the server doesn't open the `/cable` endpoint or broadcast
//! updates. Turbo will attempt to connect and fail quietly;
//! navigation + form-submit flows still work. A later task
//! wires axum's `ws` extractor to the CableServer shape.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};

use crate::db;

pub struct StartOptions<'a> {
    /// File path for the sqlite DB. Defaults to
    /// `./storage/development.sqlite3`.
    pub db_path: Option<String>,
    /// Listener port. Defaults to 3000 or `PORT` env var.
    pub port: Option<u16>,
    /// Schema SQL to apply on startup — typically
    /// `crate::schema_sql::CREATE_TABLES`.
    pub schema_sql: &'a str,
}

/// Start the server. Opens DB, applies schema, layers middleware
/// on top of the caller-supplied router, and runs axum until the
/// process exits.
pub async fn start(router: Router, opts: StartOptions<'_>) {
    let db_path = opts
        .db_path
        .unwrap_or_else(|| "storage/development.sqlite3".to_string());
    let port: u16 = opts.port.unwrap_or_else(|| {
        std::env::var("PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3000)
    });

    db::open_production_db(&db_path, opts.schema_sql);

    let app = router
        .layer(middleware::from_fn(layout_wrap))
        .layer(middleware::from_fn(method_override));

    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    println!("Roundhouse server listening on http://localhost:{}", port);
    axum::serve(listener, app).await.expect("axum serve");
}

// ── method override middleware ─────────────────────────────────

/// Rails scaffold forms submit as POST with a hidden `_method`
/// field when the real verb is PATCH / PUT / DELETE (browsers
/// don't natively support those in form elements). We consume the
/// form body, check for `_method`, rewrite the request method, and
/// re-inject the buffered body so the downstream `Form` extractor
/// still reads the params.
async fn method_override(req: Request, next: Next) -> Response {
    if req.method() != Method::POST {
        return next.run(req).await;
    }
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !content_type.starts_with("application/x-www-form-urlencoded") {
        return next.run(req).await;
    }

    let (mut parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("body too large"))
                .unwrap();
        }
    };

    // Scan for `_method=<verb>` in the urlencoded body. We only
    // look for the first hit — scaffold forms emit a single
    // _method field; more than that would be a bug upstream.
    let body_str = std::str::from_utf8(&bytes).unwrap_or("");
    let mut override_verb: Option<Method> = None;
    for pair in body_str.split('&') {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        if k == "_method" {
            let upper = v.to_ascii_uppercase();
            override_verb = match upper.as_str() {
                "PATCH" => Some(Method::PATCH),
                "PUT" => Some(Method::PUT),
                "DELETE" => Some(Method::DELETE),
                _ => None,
            };
            break;
        }
    }
    if let Some(m) = override_verb {
        parts.method = m;
    }

    let new_req = Request::from_parts(parts, Body::from(bytes));
    next.run(new_req).await
}

// ── layout wrap middleware ─────────────────────────────────────

/// Wrap HTML-typed response bodies in the document shell. Only
/// touches 2xx + 422 responses with `text/html` content type;
/// redirects pass through untouched, as do non-HTML responses (the
/// WebSocket upgrade, any JSON endpoints).
async fn layout_wrap(req: Request, next: Next) -> Response {
    let res = next.run(req).await;

    let status = res.status();
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("text/html"))
        .unwrap_or(false);

    // Pass through non-HTML + redirects.
    if !is_html {
        return res;
    }
    if status.is_redirection() {
        return res;
    }

    let (mut parts, body) = res.into_parts();
    let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("response body too large"))
                .unwrap();
        }
    };
    let inner = std::str::from_utf8(&bytes)
        .map(str::to_string)
        .unwrap_or_default();
    let wrapped = render_layout(&inner);

    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    Response::from_parts(parts, Body::from(wrapped))
}

/// The document shell. Tailwind Play CDN + plain `@hotwired/turbo`
/// (not `@hotwired/turbo-rails` — that variant auto-wires Action
/// Cable on import and we don't run `/cable`, so its transitive
/// `@rails/actioncable/src` lookup fails in the browser). Inline
/// data-URI favicon suppresses the no-icon 404 on each page load.
/// Matches the emitted scaffold layout file structurally even
/// though that file's ERB helpers still stub as TODOs — we
/// synthesize the working shell here until view-helper emission
/// catches up.
fn render_layout(body: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Roundhouse App</title>
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <link rel="icon" href="data:,">
    <script src="https://cdn.tailwindcss.com"></script>
    <script type="importmap">
    {{
      "imports": {{
        "@hotwired/turbo": "https://ga.jspm.io/npm:@hotwired/turbo@8.0.0/dist/turbo.es2017-esm.js"
      }}
    }}
    </script>
    <script type="module">import "@hotwired/turbo";</script>
  </head>
  <body>
    <main class="container mx-auto mt-8 px-5 flex flex-col">
      {}
    </main>
  </body>
</html>
"##,
        body,
    )
}
