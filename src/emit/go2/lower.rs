//! Go-specific IR→IR lowerings.
//!
//! These passes run after `src/lower/`'s target-agnostic lowerings
//! and BEFORE go2's emit walk. They rewrite IR shapes that translate
//! poorly to Go into shapes that go2's emit can produce idiomatic Go
//! from.
//!
//! ## Why Go specifically
//!
//! Go is the only currently-emitted target without a native nilable
//! scalar type or `Option`-like wrapper. The framework Ruby uses
//! nil-friendly idioms freely (`v = m[k]; if !v.nil?`,
//! `arr.first&.thing`, `ENV["FOO"] || "default"`) that translate
//! directly to Crystal (`String?`), Rust (`Option<T>`), TypeScript
//! (`T | undefined`), and Ruby itself. Go's `map[K]V` returns the
//! zero value for missing keys with no nilable wrapper, so the same
//! shapes produce `vet` errors at the nil comparison.
//!
//! Rather than build emit-time peephole logic that has to recognize
//! these patterns across statement boundaries (awkward in a bottom-
//! up emit walk), each Go-incompatible Ruby idiom gets a dedicated
//! IR→IR lowerer here. Each pass is a pure function: `Vec<LibraryClass>`
//! in, `Vec<LibraryClass>` out. Composes cleanly with the others;
//! testable in isolation via `dump_ir`-style assertions on the
//! transformed IR.
//!
//! ## Pass order
//!
//! Passes are listed in `lower_for_go` below in execution order.
//! Most should be commutative (operate on disjoint IR shapes), but
//! the orchestrator runs them sequentially so order-dependent rewrites
//! stay deterministic. When two passes overlap, document the
//! interaction at the call site.
//!
//! ## Relationship to rust2's approach
//!
//! rust2 didn't need this layer — Rust's `Option<T>` and `Result<T, E>`
//! gave each nil-prone shape a natural target form, with `str_color`
//! (an analyzer pass, not a lowerer) handling the one remaining
//! Rust-specific concern (String/&str ownership coercion). Go has no
//! analog; the rewrites have to happen IR-level. Future Go-like
//! targets (Kotlin/Swift with their own optional discipline) could
//! reuse these passes wholesale or as a starting point.

use crate::dialect::LibraryClass;

/// Apply each Go-specific lowering pass in order. Called from
/// `emit::go2::emit_overlay_files` via the `go_units` transform
/// hook — every transpiled framework class flows through this
/// pipeline before go2/emit sees it.
pub fn lower_for_go(classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
    // First (and currently only) pass: rewrite the
    // `v = m[k]; if !v.nil? { body }` pattern into a Go-friendly
    // form that go2/emit can lower to `v, ok := m[k]; if ok { ... }`.
    // Stub today — implementation lands in the follow-on session
    // once the IR-shape contract between this pass and go2/emit is
    // pinned. Identity behavior keeps the toolchain test green
    // while the scaffold is in place.
    nil_check_to_comma_ok::apply(classes)
}

/// Pattern: `v = m[k]; if !v.nil? { body using v }`.
///
/// Ruby's `Hash#[]` returns nil for missing keys, then the
/// subsequent `.nil?` guard filters. In Go, `m[k]` on `map[K]V`
/// returns the zero value of `V` for missing keys; the nilness
/// information is erased. The comma-ok form `v, ok := m[k]; if ok`
/// is Go's native equivalent of Ruby's nil check, but the rewrite
/// has to span the assignment and the conditional — too coarse for
/// the per-Send emit_expr walk.
///
/// This pass walks each method body's `Seq` looking for the
/// two-statement pattern and rewrites to a synthesized form that
/// go2/emit recognizes (final IR shape TBD — see the follow-on
/// session note in the parent module doc).
pub mod nil_check_to_comma_ok {
    use crate::dialect::LibraryClass;

    /// Identity pass for now. The real transformation walks each
    /// `MethodDef.body`, pattern-matches the
    /// `Seq[Assign{Var(v), Send(m, "[]", [k])}, If{!Send(Var(v), "nil?"), then, _}]`
    /// shape, and rewrites to a comma-ok-ready form. Implementation
    /// in the follow-on session.
    pub fn apply(classes: Vec<LibraryClass>) -> Vec<LibraryClass> {
        classes
    }
}
