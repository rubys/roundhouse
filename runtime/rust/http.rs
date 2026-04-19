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
