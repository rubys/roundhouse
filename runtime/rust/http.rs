//! Roundhouse Rust HTTP runtime — Phase 4c compile-only stubs.
//!
//! Hand-written, shipped alongside generated code (copied in by the Rust
//! emitter as `src/http.rs`). Provides just enough surface that emitted
//! controller actions type-check: `Response`, a `Params` placeholder, and
//! the free functions emitted code expects (`render`, `redirect_to`,
//! `head`, `respond_to`).
//!
//! No request parsing, no routing, no response rendering — every call
//! returns `Response::default()`. Real behavior is Phase 4e+. Controller
//! tests stay `#[ignore]` until then, so nothing in this module actually
//! runs during `cargo test`; the purpose is to make the crate compile.

#![allow(dead_code, unused_variables)]

/// Opaque response value. The real runtime will carry status + body +
/// headers; Phase 4c only needs a value that every action can return.
#[derive(Debug, Default, Clone)]
pub struct Response;

/// Controller-side view of request parameters. Bare `params` in a Ruby
/// controller lowers to `crate::http::params()` — both reads and the
/// `params.expect(...)` surface live on this stub.
pub struct Params;

impl Params {
    /// Stub for `params.expect(:key)` / `params.expect(hash)`. Generic
    /// over the expected return so emitted call sites (`Article::find(
    /// params.expect(:id))`) typecheck; the real runtime will coerce
    /// from the request body.
    pub fn expect<K, T: Default>(&self, _key: K) -> T {
        T::default()
    }
}

/// Accessor emitted for a bare `params` reference in a controller body.
pub fn params() -> Params {
    Params
}

/// Stub `render`. Accepts any single argument shape the emitter
/// produces (template symbol, string, or a keyword-arg hash lowered to
/// a `HashMap`).
pub fn render<T>(_args: T) -> Response {
    Response
}

/// Stub `redirect_to`. First arg is the target (a model, a string URL,
/// a path helper result); second is a placeholder for the Rails options
/// hash (`notice:`, `status:`, etc.). Two generics so emitted code can
/// pass whatever it built without coercion.
pub fn redirect_to<T>(_target: T) -> Response {
    Response
}

pub fn redirect_to_with<T, O>(_target: T, _opts: O) -> Response {
    Response
}

/// Stub `head :status`. Emitted when an action returns only a status.
pub fn head<T>(_status: T) -> Response {
    Response
}

/// `respond_to do |format| ... end` lowers to this: the block receives
/// a `FormatRouter` and the caller threads the format-specific Response
/// back out. Phase 4c wires only the HTML branch; the JSON branch is a
/// no-op marked `// TODO: JSON branch` at the call site.
pub fn respond_to<F>(f: F) -> Response
where
    F: FnOnce(&mut FormatRouter) -> Response,
{
    let mut fr = FormatRouter;
    f(&mut fr)
}

pub struct FormatRouter;

impl FormatRouter {
    /// HTML branch. Runs the block for its side effects and surfaces
    /// the Response.
    pub fn html<F>(&mut self, f: F) -> Response
    where
        F: FnOnce() -> Response,
    {
        f()
    }

    /// JSON branch stub — call sites route around this via a
    /// `// TODO: JSON branch` comment in Phase 4c. Kept callable so
    /// hand-written code outside the emitter still typechecks.
    pub fn json<F>(&mut self, _f: F) -> Response
    where
        F: FnOnce() -> Response,
    {
        Response
    }
}
