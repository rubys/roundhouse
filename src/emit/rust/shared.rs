//! `rust` cross-cutting helpers used by multiple emit modules.
//!
//! Phase 1 stub. Will host a port of `src/emit/rust/shared.rs`
//! (currently `emit_literal` only) plus reserved-word
//! sanitization, identifier escaping, and other small utilities
//! shared between `library.rs`, `method.rs`, `expr.rs`. Mirrors
//! `src/emit/crystal/shared.rs`.

/// Which floored Int binary op to render. Call sites pass a literal
/// variant — never a free-form method string.
#[derive(Clone, Copy)]
pub(crate) enum FloorOp {
    Div,
    Mod,
}

/// Ruby's Int / Int floors (`-7 / 2 == -4`) and Int % Int takes the
/// divisor's sign (`-7 % 3 == 2`); Rust's `/` and `%` truncate toward
/// zero. Render the floored form as a parenthesized block expression
/// (bare, a leading `{ … } + x` would parse as a statement) that
/// evaluates each operand once. `b == -1` short-circuits both ops so
/// `i64::MIN / -1` and `i64::MIN % -1` do not overflow-panic (div uses
/// `wrapping_neg`; mod is always 0).
pub(crate) fn int_floor_div_mod(op: FloorOp, lhs: &str, rhs: &str) -> String {
    match op {
        FloorOp::Div => format!(
            "({{ let (a, b): (i64, i64) = ({lhs}, {rhs}); \
             if b == -1 {{ a.wrapping_neg() }} else {{ \
             let q = a / b; \
             if a % b != 0 && ((a < 0) != (b < 0)) {{ q - 1 }} else {{ q }} }} }})"
        ),
        FloorOp::Mod => format!(
            "({{ let (a, b): (i64, i64) = ({lhs}, {rhs}); let m = \
             if b == -1 {{ 0 }} else {{ a % b }}; \
             if m != 0 && ((m < 0) != (b < 0)) {{ m + b }} else {{ m }} }})"
        ),
    }
}
