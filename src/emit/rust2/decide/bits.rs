//! Bit allocation for `Expr.decisions`.
//!
//! Layout: bits 0–31 are **cross-target** concerns — predicates whose
//! semantics other targets (TS/Crystal/Go/Kotlin/Swift) would also
//! consume if and when those targets grow a decide pass. Bits 32–63
//! are **rust2-local** — semantics tied to Rust's type system
//! (ownership, move/copy, Option wrapping for `Option<T>`-typed
//! positions).
//!
//! Adding a new bit:
//! 1. Pick the next free position in the appropriate half.
//! 2. Add the `pub const` here.
//! 3. Add a typed accessor pair (`is_<name>` / `mark_<name>`) on
//!    `Expr` in `crate::expr` if it's referenced from render.
//! 4. Set it from the appropriate decide submodule.
//!
//! Default `0` means "no decisions stamped" — render falls through
//! to its today-equivalent path, which keeps each stage of the
//! migration byte-identical until the bit is actually consumed.

// ────────────────────────────────────────────────────────────────────
// Bits 0–31: cross-target concerns
// ────────────────────────────────────────────────────────────────────

/// Stage 1. Set on a child node when the parent's operator precedence
/// would otherwise leave the child ambiguous and parens are required
/// to preserve the parse. Render wraps `(inner)` iff set.
///
/// Set by the decide pass during a top-down walk with parent
/// precedence in hand; the bit lives on the child because the
/// renderer wraps the child's output, not the parent's.
pub const NEEDS_PARENS: u64 = 1 << 0;

/// Stage 3. Set on a `Var` read that is the binding's final use in
/// the current method body. Combined with `!OWNED` and `!is_copy_ty`,
/// triggers a `.clone()` at the read site — the only safe way to read
/// a non-Copy non-owned local that's about to fall out of scope.
///
/// Conceptually cross-target — Swift `consume`, C++ `std::move`, Rust
/// last-use semantics all derive from the same analysis. Today
/// consumed only by rust2.
pub const LAST_USE: u64 = 1 << 1;

// ────────────────────────────────────────────────────────────────────
// Bits 32–63: rust2-local concerns
// ────────────────────────────────────────────────────────────────────

/// Stage 2. Set on a `Ty::Str`-typed node when the str_color
/// analysis decided the emit needs a `.to_string()` wrap — the
/// producer yields `&str`/`&'static str` (literal returned from a
/// `-> String` function, etc.) but the consumer position requires
/// owned `String`. Mutually exclusive with `STR_BORROW`.
///
/// Replaces `Expr.str_coercion = Some(StrCoercion::ToOwned)` from
/// the pre-decide-pass design. Render reads at the single point
/// `expr/mod.rs::apply_str_coercion`.
///
/// Rust-local: TS/Crystal/Python/Ruby don't distinguish owned vs
/// borrowed strings.
pub const STR_TO_OWNED: u64 = 1 << 32;

/// Stage 2. Set on a `Ty::Str`-typed node when the str_color
/// analysis decided the emit needs a `&`-prefix borrow — the
/// producer yields owned `String` but the consumer position takes
/// `&str`. Mutually exclusive with `STR_TO_OWNED`.
///
/// Replaces `Expr.str_coercion = Some(StrCoercion::Borrow)`.
pub const STR_BORROW: u64 = 1 << 33;

/// Stage 3. Set on a `Var` read where the decide pass has concluded
/// that the read site must emit `name.clone()` rather than `name`.
/// Typical rule: `is_last_use(n) && !is_owned(n) && !is_copy_ty(n.ty)`
/// → clone. Stored as a discrete bit (rather than derived at render
/// time) so the rule is centralized in decide.
///
/// Rust-local: only Rust's move semantics need this.
pub const CLONE_AT: u64 = 1 << 34;

/// Stage 4. Set on an expression whose value is being passed into a
/// position typed `Option<T>` where the source expression is typed
/// `T`. Render wraps as `Some(inner)` iff set. Centralizes the
/// "wrap with Some" decision that today is scattered across
/// `coerce.rs::coerce_arg_for_param_ty` and the field-assign coerce
/// helpers.
///
/// Rust-local: TS/Crystal use nullability rather than tagged unions.
pub const OPTION_WRAP: u64 = 1 << 35;

// ────────────────────────────────────────────────────────────────────
// Enum-valued fields (bit groups)
// ────────────────────────────────────────────────────────────────────
//
// Future stages add bit-groups for `CoerceFamily` (Stage 4) and
// `PeepholeKind` (Stage 5). Each occupies a contiguous range with
// a SHIFT + MASK pair following the precedent below:
//
// pub const COERCE_FAMILY_SHIFT: u32 = 40;
// pub const COERCE_FAMILY_MASK:  u64 = 0xF << COERCE_FAMILY_SHIFT;
//
// Left unallocated until Stage 4 lands the enum shape.
