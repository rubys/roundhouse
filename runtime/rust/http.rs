//! Roundhouse Rust HTTP runtime.
//!
//! Hand-written, shipped alongside generated code (copied in by the
//! Rust emitter as `src/http.rs`). Provides the controller-facing
//! types + helpers the emitter assumes exist: a `Params` wrapper over
//! Rails-style bracketed form parameters, a `Redirect` convenience, a
//! small `ViewCtx` carrying flash + request context to views, and
//! re-exports of axum's `Html` / `Response` / `IntoResponse` so
//! emitted action signatures can reference them through a single
//! `crate::http::*` path.
//!
//! Axum is the HTTP framework (chosen to match railcar's precedent +
//! the surrounding Rust ecosystem's gravity). Actions return `impl
//! IntoResponse`; form bodies extract via `axum::extract::Form`
//! wrapping a per-controller `#[derive(Deserialize)]` struct.

use std::collections::HashMap;

pub use axum::response::{Html, IntoResponse, Redirect, Response};

/// Wrapper over the flat HashMap form body that axum's `Form`
/// extractor produces. Rails posts nested keys like `article[title]=
/// Foo&article[body]=Bar`; this type provides the `.expect(scope,
/// &[keys])` accessor used by emitted strong-params helpers and the
/// `[key]` lookup used by `params[:id]` style access.
#[derive(Debug, Default, Clone)]
pub struct Params {
    inner: HashMap<String, String>,
}

impl Params {
    pub fn new(inner: HashMap<String, String>) -> Self {
        Self { inner }
    }

    /// Rails `params[:id]` / `params["id"]` — return the raw string
    /// value for a top-level key. Missing keys return an empty
    /// string (matches Rails' `params[:missing]` returning nil when
    /// later coerced; for Phase 4d's ID parsing, use `.int(key)`).
    pub fn get(&self, key: &str) -> &str {
        self.inner.get(key).map(|s| s.as_str()).unwrap_or("")
    }

    /// Parse a param as an `i64`. Used in place of the Ruby
    /// `params[:id]` which is string-typed but always gets coerced
    /// to an integer for DB lookup. Returns 0 on missing/unparsable.
    pub fn int(&self, key: &str) -> i64 {
        self.inner.get(key).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    /// Strong-params extractor: pull every `scope[field]` key out of
    /// the flat form body and return a new `HashMap<String, String>`
    /// keyed on `field`. Emitted strong-params helpers use this to
    /// populate their typed struct's fields.
    ///
    /// `params.expect(article: [:title, :body])` in Rails lowers to
    /// `params.expect("article", &["title", "body"])` in emitted
    /// Rust, and the returned map is consumed by the model's
    /// from-params constructor.
    pub fn expect(&self, scope: &str, keys: &[&str]) -> HashMap<String, String> {
        let prefix = format!("{scope}[");
        let mut out = HashMap::new();
        for key in keys {
            let full = format!("{prefix}{key}]");
            if let Some(v) = self.inner.get(&full) {
                out.insert((*key).to_string(), v.clone());
            }
        }
        out
    }
}

impl From<HashMap<String, String>> for Params {
    fn from(inner: HashMap<String, String>) -> Self {
        Self::new(inner)
    }
}

/// Convenience: emit `crate::http::redirect(&path)` from a path
/// helper's result. Wraps axum's `Redirect::to` with the 303 See
/// Other status that Rails uses for create/update/destroy redirects.
pub fn redirect(path: &str) -> Redirect {
    Redirect::to(path)
}

/// Convenience: emit `crate::http::html(body)` to wrap a view's
/// String output as an HTML response. Same as `Html(body)` but one
/// import shorter at call sites.
pub fn html(body: String) -> Html<String> {
    Html(body)
}

/// Error response with HTTP 422 (unprocessable entity) — Rails'
/// convention for validation failures on create/update. Emitters
/// wrap a view render in this on the `else` branch of `@model.save`.
pub fn unprocessable(body: String) -> (axum::http::StatusCode, Html<String>) {
    (axum::http::StatusCode::UNPROCESSABLE_ENTITY, Html(body))
}

/// Context threaded through view functions. Phase 4d minimum: flash
/// notice (read in every view via `notice.present?`). Later: current
/// user, csrf token, request path, locale, etc.
#[derive(Debug, Default, Clone)]
pub struct ViewCtx {
    pub notice: Option<String>,
}

impl ViewCtx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_notice(notice: impl Into<String>) -> Self {
        Self { notice: Some(notice.into()) }
    }
}

// ── rust2 controller-action response state ──────────────────────
//
// Rails controllers thread response data through implicit state —
// `render`, `redirect_to`, `head`, and `response.headers[…] = …`
// each accumulate into the controller's response object, which the
// framework serializes to the HTTP body after the action returns.
// Rust2's emit shape carries the controller as `impl X { pub fn
// show(&mut self) }` — `&mut self` methods returning `()`. That
// signature isn't compatible with axum's free-fn-extractor-then-
// IntoResponse contract.
//
// Bridge: emit per-action axum wrapper free fns that clear this
// thread-local, build the controller, call the action, then
// translate the accumulated `ControllerResponse` into an
// `axum::response::Response`. The AC::Base shim's `render` /
// `render_with` / `redirect_to` / `head` methods (today no-ops
// emitted at `src/emit/rust2.rs:~782`) become thin writers to
// this state.
//
// Per-thread because axum dispatches each request on a tokio task
// that's pinned to one thread for the duration of an action body
// (controller bodies are sync `&mut self` methods — no `.await`
// inside, so thread affinity holds). A future migration to async
// action bodies would need a per-task storage shape (extension
// types, task_local!, etc.).

#[derive(Debug, Clone)]
pub struct ControllerResponse {
    pub status: u16,
    pub body: String,
    pub content_type: String,
    /// Set when `redirect_to` fires; the wrapper emits a 3xx with
    /// this as the `Location` header instead of an HTML body.
    pub location: Option<String>,
}

impl Default for ControllerResponse {
    fn default() -> Self {
        Self {
            status: 200,
            body: String::new(),
            content_type: "text/html; charset=utf-8".to_string(),
            location: None,
        }
    }
}

thread_local! {
    static RESPONSE: std::cell::RefCell<ControllerResponse> =
        std::cell::RefCell::new(ControllerResponse::default());
    static REQUEST_FORMAT: std::cell::RefCell<String> =
        std::cell::RefCell::new(String::from("html"));
}

/// Request extension carrying the inferred format ("html"/"json").
/// Set by the `request_format_middleware` in `server.rs` after it
/// strips a `.json` suffix off the URI; read by the per-action axum
/// wrappers (extracted via `axum::extract::Extension<RequestFormatExt>`)
/// and threaded into the thread-local before the controller body runs.
#[derive(Clone, Debug)]
pub struct RequestFormatExt(pub String);

/// Stash the inferred format on the per-task thread-local. The axum
/// wrapper calls this synchronously immediately before the controller
/// action body — `AC::Base#request_format` (emitted as a shim method
/// on each controller) reads it back via `request_format_get`. No
/// `.await` between set and read, so thread affinity holds.
pub fn request_format_set(format: String) {
    REQUEST_FORMAT.with(|r| *r.borrow_mut() = format);
}

/// Read the current request's format. Called by the controller-shim
/// `request_format()` method; defaults to `"html"` if no middleware
/// has populated it (e.g. unit tests instantiating a controller
/// directly).
pub fn request_format_get() -> String {
    REQUEST_FORMAT.with(|r| r.borrow().clone())
}

/// Tag every request with its inferred format ("html" or "json") as
/// an extension before it reaches the per-action handler. The
/// emitted router attaches this as a `.layer()` so both `axum::serve`
/// (production) and `axum_test::TestServer` (controller tests) share
/// one wiring path.
///
/// Why an Extension and not a URI rewrite: in axum 0.8 `Router::layer`
/// wraps each route's handler — route matching + `Path<...>`
/// extraction happens *before* the layer runs, so URI rewrites here
/// are too late to affect routing. The router emit registers explicit
/// `.json`-suffixed entries for parameterless paths
/// (`src/emit/rust2.rs::render_axum_router_body`); parameterized
/// paths capture the `.json` tail as part of the segment (e.g.
/// `id="1.json"`), and the action wrapper strips the suffix before
/// parsing the id as `i64`. This layer just surfaces the inferred
/// format so the `if self.request_format() == "json"` branch dispatches
/// the JSON jbuilder view.
pub async fn request_format_middleware(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let format = if req.uri().path().ends_with(".json") {
        "json"
    } else {
        "html"
    };
    req.extensions_mut().insert(RequestFormatExt(format.to_string()));
    next.run(req).await
}

/// Reset the thread-local to defaults. Called at the top of each
/// axum wrapper so a prior action's state doesn't leak into the
/// current request.
pub fn response_clear() {
    RESPONSE.with(|r| *r.borrow_mut() = ControllerResponse::default());
}

/// `render(content)` — stash the body string. Defaults already
/// have 200/text-html, so a bare render is fully wired.
pub fn response_set_body(body: String) {
    RESPONSE.with(|r| r.borrow_mut().body = body);
}

/// `render_with(content, opts)` — body + content_type, optionally
/// status. Honors common `opts` keys (`content_type`, `status`).
/// Unknown keys ignored; the AC::Base shim's call site already
/// strips the Ruby-only knobs.
pub fn response_set_body_with(body: String, content_type: Option<String>, status: Option<u16>) {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        resp.body = body;
        if let Some(ct) = content_type {
            resp.content_type = ct;
        }
        if let Some(st) = status {
            resp.status = st;
        }
    });
}

/// `redirect_to(path, opts)` — 303 See Other by default; the
/// `status: :see_other` opt matches Rails' default convention for
/// post-mutation redirects (avoids form re-submit on back/refresh).
pub fn response_set_redirect(location: String, status: u16) {
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        resp.status = status;
        resp.location = Some(location);
        resp.body = String::new();
    });
}

/// `head(name, opts)` — Rails-style status symbol → numeric code.
/// Body stays empty. Symbol names mirror `Rack::Utils::SYMBOL_TO_STATUS_CODE`.
pub fn response_set_head(status_name: &str, content_type: Option<String>) {
    let code = status_name_to_code(status_name);
    RESPONSE.with(|r| {
        let mut resp = r.borrow_mut();
        resp.status = code;
        resp.body = String::new();
        if let Some(ct) = content_type {
            resp.content_type = ct;
        }
    });
}

/// Snapshot + reset — used by the per-action axum wrapper to read
/// out the state immediately after the action returns. Returns
/// owned value so the borrow on the thread-local is short.
pub fn response_take() -> ControllerResponse {
    RESPONSE.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// Translate a thread-local response into an `axum::response::Response`.
/// Redirect-shaped state produces a 3xx with `Location`; otherwise
/// emits the body with the recorded content-type + status.
pub fn response_into_axum(resp: ControllerResponse) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);
    if let Some(location) = resp.location {
        let mut response = (status, ()).into_response();
        if let Ok(hv) = axum::http::HeaderValue::from_str(&location) {
            response.headers_mut().insert(axum::http::header::LOCATION, hv);
        }
        return response;
    }
    let body = resp.body;
    let content_type = resp.content_type;
    let mut response = (status, body).into_response();
    if let Ok(hv) = axum::http::HeaderValue::from_str(&content_type) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, hv);
    }
    response
}

/// Public alias for `status_name_to_code` — exposed for the AC::Base
/// shim emitted in `src/emit/rust2.rs`, which reaches it from
/// outside the crate-private `http` module. Same semantics; just
/// a re-export that survives module privacy.
pub fn status_name_to_code_pub(name: &str) -> u16 {
    status_name_to_code(name)
}

/// Ruby `Object#to_s` analog. Rails' `inner_v.to_s` in
/// `ActionView::ViewHelpers#render_attrs` ships through any
/// Hash[String, untyped]; on strict-typed targets the `untyped`
/// alias resolves to `serde_json::Value`, whose `Display` /
/// `to_string()` emits a JSON serialization (so
/// `Value::String("reload").to_string()` becomes `"\"reload\""`,
/// not `reload`). Ruby's `String#to_s` is identity — bare string.
///
/// `RubyToS` bridges: implementations cover the three recv types
/// rust2 emit lowers `untyped`-receiver `.to_s` Sends to (`str` /
/// `String` / `serde_json::Value`). Rust resolves the impl at
/// compile time via auto-deref, so the rust2 dispatch can emit
/// `(recv).ruby_to_s()` uniformly without distinguishing closure
/// params (genuinely `&String` at runtime, body-typer marks
/// `Untyped`) from value-typed locals (genuinely `&Value`).
///
/// Used at every call site in `runtime/ruby/action_view/view_helpers.rb`
/// that produces an attribute / data-attribute / link tag value;
/// the lowered IR's `.to_s` Sends on Untyped recvs route through
/// this trait by the recv-Ty-aware bridge in
/// `src/emit/rust2/expr/send/dispatch.rs`.
pub trait RubyToS {
    fn ruby_to_s(&self) -> String;
}

impl RubyToS for str {
    fn ruby_to_s(&self) -> String {
        self.to_string()
    }
}

impl RubyToS for String {
    fn ruby_to_s(&self) -> String {
        self.clone()
    }
}

impl RubyToS for serde_json::Value {
    fn ruby_to_s(&self) -> String {
        match self {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        }
    }
}

/// Translate a flat axum-`Form<HashMap<String, String>>` body into
/// the nested params shape Rails controllers expect. Form names of
/// the shape `article[title]=Foo` land at `params["article"]
/// ["title"] = "Foo"`; top-level names pass through unchanged.
///
/// One level of bracket-nesting only — scaffold blog forms don't
/// reach deeper. The lowered `<Resource>Params::from_raw` factory
/// always looks up a single nested scope (`params.get(resource)`)
/// then individual fields under it, so the single-level shape
/// covers every emitted call site today. Deep nesting
/// (`comment[article_attributes][title]`) becomes a follow-on if
/// `accepts_nested_attributes_for` lands.
pub fn params_from_form(
    form: HashMap<String, String>,
) -> HashMap<String, serde_json::Value> {
    let mut out: HashMap<String, serde_json::Value> = HashMap::new();
    for (k, v) in form {
        if let (Some(open), Some(close)) = (k.find('['), k.rfind(']')) {
            if close > open {
                let scope = &k[..open];
                let inner = &k[open + 1..close];
                let entry = out
                    .entry(scope.to_string())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                if let serde_json::Value::Object(map) = entry {
                    map.insert(inner.to_string(), serde_json::Value::from(v));
                    continue;
                }
            }
        }
        out.insert(k, serde_json::Value::from(v));
    }
    out
}

/// Rails status-symbol → HTTP code. Subset matching the names the
/// scaffold emit reaches (`:ok`, `:no_content`, `:not_found`,
/// `:unprocessable_entity`, `:see_other`). Unknown names fall back
/// to 200 OK — the controller path that emits an unknown symbol is
/// generally a bug the caller will see via behavior, not a route
/// the framework should silently 500 on.
fn status_name_to_code(name: &str) -> u16 {
    match name {
        "ok" => 200,
        "created" => 201,
        "accepted" => 202,
        "no_content" => 204,
        "moved_permanently" => 301,
        "found" => 302,
        "see_other" => 303,
        "not_modified" => 304,
        "temporary_redirect" => 307,
        "permanent_redirect" => 308,
        "bad_request" => 400,
        "unauthorized" => 401,
        "forbidden" => 403,
        "not_found" => 404,
        "unprocessable_entity" | "unprocessable_content" => 422,
        "internal_server_error" => 500,
        _ => 200,
    }
}
