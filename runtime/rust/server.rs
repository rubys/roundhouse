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
//! `start` also mounts `GET /cable` onto the axum router, handing
//! the upgrade off to `crate::cable::cable_handler`. The route is
//! always registered — apps that don't use Turbo Streams simply
//! never receive a client connection, and the handler is cheap
//! (one OnceLock hashmap check on subscribe).

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Router,
};
use tower_http::services::ServeDir;

use crate::cable;
use crate::db;
use crate::view_helpers;

pub struct StartOptions<'a> {
    /// File path for the sqlite DB. Defaults to
    /// `./storage/development.sqlite3`.
    pub db_path: Option<String>,
    /// Listener port. Defaults to 3000 or `PORT` env var.
    pub port: Option<u16>,
    /// Schema SQL to apply on startup — typically
    /// `crate::schema_sql::CREATE_TABLES`.
    pub schema_sql: &'a str,
    /// Layout renderer — the emitted `render_layouts_application`
    /// (or equivalent). Called after each non-redirect response
    /// with the inner view body already stashed via
    /// `view_helpers::set_yield`. When `None`, the layout-wrap
    /// middleware falls back to the minimal synthesized shell
    /// below. Applies to apps that don't emit a layouts/
    /// application ERB template (e.g. tiny-blog).
    pub layout: Option<fn() -> String>,
}

/// Process-wide layout renderer, set by `start`. Read by the
/// `layout_wrap` middleware. Axum middleware fns can't capture
/// runtime state cleanly without boxing + extensions, so we use
/// a static slot — the server runs one app per process.
static LAYOUT_FN: std::sync::OnceLock<fn() -> String> = std::sync::OnceLock::new();

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

    if let Some(layout) = opts.layout {
        let _ = LAYOUT_FN.set(layout);
    }

    db::open_production_db(&db_path, opts.schema_sql);

    // Static assets: serve `static/assets/<name>` for `/assets/*`
    // requests via tower-http's ServeDir. Mirrors Rails' Propshaft URL
    // shape — the importmap pins and `stylesheet_link_tag("tailwind")`
    // both point at /assets/<name>. `bin/rh transpile rust` writes the
    // actual files (Tailwind compile output, turbo.min.js copy) into
    // `static/assets/`. ServeDir returns 404 on miss without consuming
    // the request, so other routes still resolve normally.
    let app = router
        .nest_service("/assets", ServeDir::new("static/assets"))
        .route("/cable", get(cable::cable_handler))
        .layer(middleware::from_fn(layout_wrap))
        .layer(middleware::from_fn(method_override))
        .layer(middleware::from_fn(request_format));

    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    println!("Roundhouse server listening on http://localhost:{}", port);
    axum::serve(listener, app).await.expect("axum serve");
}

// ── request format middleware ──────────────────────────────────

/// Strip a `.json` suffix off the request URI before route matching
/// and stash the inferred format ("html" or "json") in a request
/// extension. The per-action axum wrappers (emitted by `rust2.rs::
/// render_axum_handler_wrappers`) extract the extension and thread
/// it into the controller via `crate::http::request_format_set`, so
/// emitted `if self.request_format() == "json"` branches dispatch
/// to JSON jbuilder views.
///
/// Outermost layer so the URI rewrite happens before axum's matcher
/// sees the path — `/articles.json` and `/articles` share one route
/// entry, matching the TS / Go / Crystal / Ruby shape.
async fn request_format(mut req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let (format, new_path) = if let Some(stripped) = path.strip_suffix(".json") {
        ("json", Some(stripped.to_string()))
    } else {
        ("html", None)
    };

    if let Some(new_path) = new_path {
        let pq = req
            .uri()
            .query()
            .map(|q| format!("{new_path}?{q}"))
            .unwrap_or(new_path);
        // Rebuild Uri with the suffix-stripped path. Authority/scheme
        // are absent on the per-request URI axum hands us (only path
        // + query), so a path-and-query Uri is sufficient.
        if let Ok(new_uri) = pq.parse::<Uri>() {
            *req.uri_mut() = new_uri;
        }
    }

    req.extensions_mut()
        .insert(crate::http::RequestFormatExt(format.to_string()));

    next.run(req).await
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
    // Wipe any stale yield/slot state before the handler runs.
    // Axum's multi-thread runtime means each worker thread has
    // its own thread-local; reset covers the current worker.
    view_helpers::reset_render_state();

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

    // If the app has an emitted layout (`opts.layout` was set in
    // `start`), stash the inner body for `<%= yield %>` and invoke
    // the layout. Otherwise fall back to the minimal synthesized
    // shell so apps without an ERB layout still render.
    let wrapped = if let Some(layout) = LAYOUT_FN.get() {
        view_helpers::set_yield(&inner);
        layout()
    } else {
        render_layout(&inner)
    };

    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    Response::from_parts(parts, Body::from(wrapped))
}

/// The document shell. Asset paths point at `/assets/tailwind.css`
/// + `/assets/turbo.min.js`, served by `tower-http`'s `ServeDir`
/// mounted on `/assets` (see `start`). `bin/rh transpile rust` is
/// expected to have populated `static/assets/` with the Tailwind
/// compile output + a copy of turbo.min.js; without that step the
/// page is unstyled but functional. Plain `@hotwired/turbo` (not
/// `@hotwired/turbo-rails`) avoids the latter's transitive
/// `@rails/actioncable/src` lookup, which would 404 in the browser
/// — our cable handler at `/cable` matches turbo's default URL.
/// Inline data-URI favicon suppresses the no-icon 404 on each
/// page load. Used only as a fallback when no emitter layout is
/// supplied via `StartOptions::layout`; the emitted Layouts
/// module overrides this for apps that have `app/views/layouts/
/// application.{erb,rb}`.
fn render_layout(body: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Roundhouse App</title>
    <meta name="viewport" content="width=device-width,initial-scale=1">
    <link rel="icon" href="data:,">
    <link rel="stylesheet" href="/assets/tailwind.css">
    <script type="importmap">
    {{
      "imports": {{
        "@hotwired/turbo": "/assets/turbo.min.js"
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
