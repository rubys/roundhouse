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
mod last_use;
mod parens;
pub mod str_color;

use crate::dialect::LibraryClass;

/// Run the decide pass over a class slice. Mirrors the signature of
/// `analyze::str_color::color_classes` and `analyze::mutates_self::
/// propagate` so it can be wired into the same runtime-units transform
/// and any future app-code annotation site. Mutates the bodies in
/// place — render reads the stamped bits later.
///
/// Per-concern submodules run in sequence. Each stamps an
/// independent bit family; ordering doesn't matter today because no
/// two concerns share a bit. New stages append calls here.
pub fn decide_classes(classes: &mut [LibraryClass]) {
    parens::stamp(classes);
    last_use::stamp(classes);
}
