//! `rust2` decide pass — computes per-node decisions and stamps them
//! onto `Expr.decisions` bits before render walks the same IR.
//!
//! See issue #22 for the full architectural rationale. The split:
//!
//! - **decide** (this module): walks IR, computes ownership / Option-
//!   wrap / clone / parens / coerce-family / peephole-kind decisions,
//!   stamps them onto the IR via the bit constants in `bits.rs`.
//! - **render** (`super::expr`, `super::method`, `super::library`):
//!   reads bits, produces Rust source. No Ty inspection beyond
//!   rendering what decide already chose.
//!
//! Stage 0 (this commit): module skeleton + no-op `decide_classes`
//! entry point + bit constants + typed accessors on `Expr`. Wired
//! into the runtime-units transform alongside `color_classes` and
//! `mutates_self::propagate` so subsequent stages can stamp bits
//! without plumbing changes. Default `Expr.decisions == 0` means
//! render sees no bits set and behaves identically to today's emit.
//!
//! Stages 1–5 each migrate one decision family into this pass; see
//! `bits.rs` for the bit allocation and roundhouse#22 for phasing.

pub mod bits;

use crate::dialect::LibraryClass;

/// Run the decide pass over a class slice. Mirrors the signature of
/// `analyze::str_color::color_classes` and `analyze::mutates_self::
/// propagate` so it can be wired into the same runtime-units transform
/// and any future app-code annotation site. Mutates the bodies in
/// place — render reads the stamped bits later.
///
/// Stage 0: no-op. Subsequent stages add per-concern submodules
/// (`parens.rs`, `str_color.rs`, etc.) and dispatch them from here.
pub fn decide_classes(_classes: &mut [LibraryClass]) {
    // Stage 0 placeholder. Each subsequent stage adds a per-concern
    // walker call here that traverses method bodies and stamps the
    // appropriate bit on each matching node.
}
