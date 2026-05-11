//! `rust2` method emit — `MethodDef` → Rust `fn` rendering.
//!
//! Phase 1 stub. Mirrors `src/emit/crystal/method.rs`: drives off
//! `MethodDef.signature` (typed annotations vs untyped fallback),
//! handles `self` vs `&self` vs `&mut self` receiver shape,
//! lifetimes for borrowed params. Lifts/ports the generic emit
//! infrastructure currently buried in `src/emit/rust/controller.rs`
//! (`EmitCtx`, `emit_body`, `emit_stmt`) during Phase 5.
