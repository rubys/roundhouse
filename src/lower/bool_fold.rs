//! Literal boolean short-circuit fold: `false && rhs` → `false`,
//! `true || rhs` → `true`.
//!
//! Ruby never evaluates the right-hand side of these — it is provably
//! dead — but AOT targets compile it regardless, so a
//! deliberately-disabled expression keeps its whole unresolvable tail
//! alive (lobsters-bench turns page caching off as
//! `CACHE_PAGE = proc { false && @user.blank? && ... }`; the dead
//! `blank?`-on-untyped tail was a hard compile stop under spinel).
//! Folding to the literal is semantics-identical — same value, and the
//! discarded side could never run — and deletes the dead code for every
//! target at once.
//!
//! Only a LITERAL false/true left operand folds; the value-preserving
//! rewrites for the other polarity (`true && x` → `x`) are left alone —
//! they don't remove dead code, and the surviving `x` keeps the source
//! shape. Runs first in the post-analyze order so the blank pass never
//! ledgers residue for code this fold deletes.

use crate::app::App;
use crate::expr::{BoolOpKind, Expr, ExprNode, Literal};

pub fn apply_bool_fold_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut fold);
}

fn fold(e: &mut Expr) {
    // Children first, so a chain folds outward: `(false && a) && b`
    // becomes `false && b` and then `false`.
    e.node.for_each_child_mut(&mut fold);
    let ExprNode::BoolOp { op, left, .. } = &*e.node else { return };
    let ExprNode::Lit { value: Literal::Bool { value } } = &*left.node else { return };
    let dead_rhs = match op {
        BoolOpKind::And => !*value,
        BoolOpKind::Or => *value,
    };
    if !dead_rhs {
        return;
    }
    let span = left.span;
    let folded = *value;
    *e = Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: folded } });
    e.ty = Some(crate::ty::Ty::Bool);
}
