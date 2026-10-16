//! Unary minus and plus on a number → the arithmetic every target spells.
//!
//! Prism folds `-5` into a negative literal, but `-a` on a local is a
//! send of `-@` to `a` (and `+a` one of `+@`). The analyzer types those
//! as the receiver's numeric type, and no non-Ruby emitter has a prefix
//! operator for a method named `-@`: each wrote it as a method call,
//! `a.-@` in TypeScript and Python and `a.-@()` in Rust, none of which
//! parse. Lowering once here covers every target with vocabulary they
//! already speak:
//!
//! - `-x` → `x * -1`. Not `0 - x`: for a Float that answers `0.0` where
//!   Ruby's `-0.0` keeps the sign. Multiplying by `-1` matches `-@` for
//!   every Integer and Float, including `-0.0`, infinities and NaN.
//! - `+x` → `x`. `Numeric#+@` answers the receiver itself.
//!
//! Only a receiver typed Integer or Float is rewritten. Any other
//! receiver may define `-@` itself, and that call is left alone.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_numeric_unary_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    super::for_each_test_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if !args.is_empty() {
        return;
    }
    let ty = match recv.ty {
        Some(Ty::Int) => Ty::Int,
        Some(Ty::Float) => Ty::Float,
        _ => return,
    };
    match method.as_str() {
        "+@" => {
            let recv = recv.clone();
            *expr = recv;
        }
        "-@" => {
            let recv = recv.clone();
            let value = if ty == Ty::Float {
                Literal::Float { value: -1.0 }
            } else {
                Literal::Int { value: -1 }
            };
            let mut lit = Expr::new(expr.span, ExprNode::Lit { value });
            lit.ty = Some(ty.clone());
            *expr.node = ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("*"),
                args: vec![lit],
                block: None,
                parenthesized: false,
            };
            expr.ty = Some(ty);
        }
        _ => {}
    }
}
