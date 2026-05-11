//! `rust2` expression emit — `Expr` IR → Rust source-text.
//!
//! Phase 1 stub. Mirrors `src/emit/crystal/expr.rs` in role:
//! recursive walker over `ExprNode`, producing Rust syntax. The bulk
//! of the work is `emit_send` (method dispatch translation, including
//! Ruby→Rust stdlib bridges like `String#start_with?` →
//! `str::starts_with`, `Hash#fetch(K, nil)` → `HashMap::get`/`Option`,
//! etc.). Ports the generic emit_expr/emit_send/rewrite_ruby_dot_call
//! / apply_rust_chain_modifier from `src/emit/rust/controller.rs`
//! during Phase 5.
