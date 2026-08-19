//! ActiveSupport byte-size helpers → the multiplication they are.
//!
//! `5.megabytes` → `5 * 1048576`. Unlike the sibling DURATION helpers
//! (`60.seconds`), which Rails answers with an `ActiveSupport::Duration`
//! value object that the runtime has to model, `Numeric#megabytes` is
//! defined as literally `self * 1024 * 1024` — the result is an ordinary
//! Integer (or Float, for a Float receiver) with no wrapper and no unit
//! memory. So the grounded form is the arithmetic itself, and no
//! runtime support is needed on any target.
//!
//! Two things this buys beyond "it stops raising NoMethodError on a
//! target without ActiveSupport":
//!
//! - The site types `Int` instead of `Ty::Untyped`
//!   (`analyze::body::send::int_method` files these under "a
//!   Numeric-ish value we don't model structurally"), so every
//!   downstream consumer of the value keeps a real type. That matters
//!   most for the strict targets, which cannot emit an untyped slot.
//! - It reaches CLASS-BODY CONSTANTS, which is where the corpus
//!   actually writes them (`Opengraph::Fetch::MAX_BODY_SIZE =
//!   5.megabytes`) — load-time code, so an ungrounded send there kills
//!   the process at `require`, not at first call.
//!
//! ONE AMBIGUITY, and it is the reason this checks the receiver type:
//! `bytes` is also `String#bytes`, which answers an Array of byte
//! values — a completely different meaning on a receiver the corpus
//! also holds. A receiver whose type we cannot see is left alone rather
//! than guessed at; a missed fold degrades to the status quo, a wrong
//! one silently turns a byte array into a number.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_byte_size_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    // TEST bodies too, for the same reason `duration` and `time_current`
    // walk them: this rewrite produces an integer literal, vocabulary
    // every target already speaks, rather than leaning on a CRuby
    // overlay the strict-target test lanes do not ship. campfire's
    // `opengraph_fetch_test` builds a `1.gigabyte` body to check the
    // size cap, and a bare `gigabyte` send is a NoMethodError there
    // exactly as it would be in a model.
    super::for_each_test_body(app, &mut rewrite);
}

/// Bytes per unit, as ActiveSupport defines them — binary (1024-based),
/// not SI.
fn factor(method: &str) -> Option<i64> {
    const KB: i64 = 1024;
    Some(match method {
        "byte" | "bytes" => 1,
        "kilobyte" | "kilobytes" => KB,
        "megabyte" | "megabytes" => KB.pow(2),
        "gigabyte" | "gigabytes" => KB.pow(3),
        "terabyte" | "terabytes" => KB.pow(4),
        "petabyte" | "petabytes" => KB.pow(5),
        "exabyte" | "exabytes" => KB.pow(6),
        _ => return None,
    })
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let Some(mult) = byte_factor(expr) else { return };

    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
    let recv = recv.take().expect("byte_factor matched a receiver");
    let ty = recv.ty.clone();
    let span = expr.span;

    // `1.byte` is the identity — emit the receiver, not `x * 1`.
    if mult == 1 {
        *expr = recv;
        return;
    }

    let mut lit = Expr::new(span, ExprNode::Lit { value: Literal::Int { value: mult } });
    lit.ty = Some(Ty::Int);
    *expr.node = ExprNode::Send {
        recv: Some(recv),
        method: Symbol::from("*"),
        args: vec![lit],
        block: None,
        parenthesized: false,
    };
    // A Float receiver keeps Float (`1.5.megabytes` is a Float under
    // Rails too); everything else is the Int the fold produces.
    expr.ty = Some(if ty == Some(Ty::Float) { Ty::Float } else { Ty::Int });
}

/// The multiplier for a numeric-receiver byte-size call, or `None` when
/// this is not one.
fn byte_factor(expr: &Expr) -> Option<i64> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    if !is_numeric(recv) {
        return None;
    }
    factor(method.as_str())
}

/// Is this receiver definitely a number? — see the `String#bytes` note
/// in the header for why nothing else qualifies.
///
/// The LITERAL arm is not redundant with the type arm: a class-body
/// constant's initializer carries no type stamp (analyze walks method
/// bodies, not constants), and constants are exactly where the corpus
/// writes these — so a type-only gate would miss every real site.
fn is_numeric(recv: &Expr) -> bool {
    matches!(recv.ty, Some(Ty::Int) | Some(Ty::Float))
        || matches!(
            &*recv.node,
            ExprNode::Lit { value: Literal::Int { .. } | Literal::Float { .. } }
        )
}
