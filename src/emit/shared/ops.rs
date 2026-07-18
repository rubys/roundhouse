//! Shared binary-operator dispatch.
//!
//! Several targets (swift, kotlin, csharp) share the same decision about
//! a one-arg send with a receiver: is the method a native infix operator
//! (`a + b`, `a < b`, …), a collection append (`<<` / `push`), or neither?
//! The *decision* is identical; only the append syntax differs per target
//! (`.append` / `.add` / `.Add`). This classifier makes the decision;
//! each target renders its own syntax.

/// The classification of a one-arg send (`recv.method(arg)`) for the
/// shared binary-operator dispatch. The caller has already established
/// `recv.is_some()` and `args.len() == 1`.
pub enum BinopCase<'a> {
    /// A native infix operator whose Ruby method name is also its target
    /// spelling — render `lhs op rhs`. Carries the operator text.
    NativeInfix(&'a str),
    /// `<<` / `push` — append to a collection. Each target names the
    /// append method differently, so only the case is shared.
    Append,
    /// Not a shared binary-operator form; the caller continues its own
    /// per-target dispatch.
    NotBinop,
}

/// Classify a send's `method` for shared binop dispatch. Intended to be
/// called inside a `(Some(recv), 1 arg)` guard — arity and receiver
/// presence are the caller's responsibility.
pub fn classify_binop(method: &str) -> BinopCase<'_> {
    match method {
        "+" | "-" | "*" | "/" | "%" | "<" | ">" | "<=" | ">=" | "==" | "!=" | "&&" | "||" => {
            BinopCase::NativeInfix(method)
        }
        "<<" | "push" => BinopCase::Append,
        _ => BinopCase::NotBinop,
    }
}
