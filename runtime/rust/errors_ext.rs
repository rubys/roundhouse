//! Per-target error/raise primitives the transpiled framework runtime
//! reaches via bare-token emit.
//!
//! `raise(KIND, payload)` is the Ruby-shape `raise Klass, "..."`
//! emitted by the transpile pipeline. Rust has no `raise` keyword and
//! the trio of error classes (`NotImplementedError`, `RecordNotFound`,
//! `RecordInvalid`) doesn't transpile cleanly yet — Display + Error
//! synthesis for `class < StandardError` is a separate emit feature.
//!
//! Phase 3 stub: a single `FrameworkError` enum, three module-level
//! consts the transpile's bare tokens map to (via the `imports` field
//! in `RUST_RUNTIME`), and a `raise` function generic over the payload
//! type. The function returns `!` so call sites in non-Unit-returning
//! methods compile (`!` coerces to any type).
//!
//! Payload is intentionally `T` (no Debug/Display bound) because the
//! emitted code passes structs (`self`, `Base` instances) that don't
//! derive Debug yet. Lost message content is acceptable for the
//! contract-marker raise calls (table_name, instantiate, etc.) — the
//! panic still surfaces the kind, which is enough to diagnose a
//! missing subclass override.

#[derive(Debug, Clone, Copy)]
pub enum FrameworkError {
    NotImplemented,
    RecordNotFound,
    RecordInvalid,
}

#[allow(non_upper_case_globals)]
pub const NotImplementedError: FrameworkError = FrameworkError::NotImplemented;
#[allow(non_upper_case_globals)]
pub const RecordNotFound: FrameworkError = FrameworkError::RecordNotFound;
#[allow(non_upper_case_globals)]
pub const RecordInvalid: FrameworkError = FrameworkError::RecordInvalid;

/// Ruby-shape `raise Klass, payload`. Panics with the framework
/// error kind; payload is accepted but discarded.
///
/// Returns `!` so the caller can use it as the body of any-typed
/// method (`fn table_name() -> String { raise(...) }` compiles).
pub fn raise<T>(kind: FrameworkError, _payload: T) -> ! {
    panic!("FrameworkError::{:?}", kind);
}

/// Placeholder for Ruby's `self.name` (class name) inside emitted
/// class methods. The transpile lowers bare `name` calls to a free
/// function reference; until the emit-side rewrites these to
/// per-class string literals or `std::any::type_name::<Self>()`,
/// this stub gives the references something to resolve to.
pub fn name() -> &'static str {
    "Base"
}
