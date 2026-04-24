//! Cross-target emitter helpers.
//!
//! Logic used by more than one per-target emitter (`crystal/`, `go/`, …)
//! lives here — classifiers that reduce a shared decision to a small
//! enum, renderers that produce target-neutral text the emitters embed
//! verbatim, etc. This keeps the top of `emit/` as one entry per target
//! plus this module, and gives a clear home for shared code that
//! doesn't fit the structured-to-structured lowering pattern in
//! `crate::lower`.

pub mod add;
pub mod cmp;
pub mod div_pow;
pub mod eq;
pub mod modulo;
pub mod mul;
pub mod schema_sql;
pub mod sub;
