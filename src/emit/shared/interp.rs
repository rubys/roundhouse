//! Shared string-interpolation walker.
//!
//! Every target renders a `StringInterp` the same way structurally — a
//! quote, then alternating escaped text chunks and delimited embedded
//! expressions, then a closing quote. The per-target deltas are exactly
//! two: the interpolation *delimiters* (`#{}`, `${}`, `{}`) and the
//! text *escape table*. This walker takes both as parameters so the
//! shared structure lives once; the escape function stays per-target
//! (deliberately — see the `escape_str` note in the maintainability
//! plan, 3.3: the escape tables differ in target-semantic characters).
//!
//! Targets whose expression arm is not a plain `open + expr + close`
//! (swift wraps optionals in `RhString.s(...)`; go2 appends to a
//! variable) render their interpolations directly and don't use this.

use crate::expr::{Expr, InterpPart};

/// The literal fragments that bracket an interpolated string in a given
/// target's syntax.
pub struct InterpDelims<'a> {
    /// Opening quote, including any string-prefix (`"\""`, C#'s `"$\""`).
    pub open_quote: &'a str,
    /// Closing quote (`"\""` for every current target).
    pub close_quote: &'a str,
    /// Text before an embedded expression (`"#{"`, `"${"`, `"{"`).
    pub expr_open: &'a str,
    /// Text after an embedded expression (`"}"`).
    pub expr_close: &'a str,
}

/// Render a `StringInterp`'s `parts` into a target string literal.
/// `emit_expr` renders one embedded expression; `escape` escapes one
/// literal text chunk for the target's quoted-string body.
pub fn render(
    parts: &[InterpPart],
    delims: &InterpDelims,
    emit_expr: impl Fn(&Expr) -> String,
    escape: impl Fn(&str) -> String,
) -> String {
    let mut out = String::from(delims.open_quote);
    for part in parts {
        match part {
            InterpPart::Text { value } => out.push_str(&escape(value)),
            InterpPart::Expr { expr } => {
                out.push_str(delims.expr_open);
                out.push_str(&emit_expr(expr));
                out.push_str(delims.expr_close);
            }
        }
    }
    out.push_str(delims.close_quote);
    out
}
