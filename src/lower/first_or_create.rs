//! `Model.where(k: v, …).first_or_create` / `.first_or_initialize`
//! grounding: macro-inline the find-else-build at the call site,
//! seeding the built record from the where-clause equality pairs —
//! Rails' contract (lobsters builds `ReadRibbon.where(user:,
//! story:).first_or_create` and reads the ribbon back; a blank-built
//! record would drop the keys). The heterogeneous conditions hash is
//! exactly the shape the macro-inline line says to expand rather than
//! push through a runtime helper: inlined, each pair lands as a typed
//! setter send.
//!
//!   _rec = Model.where(k: v).first
//!   if _rec.nil?
//!     _rec = Model.new
//!     _rec.k = v
//!     _rec.save            # first_or_create only
//!   end
//!   <original consumer of the value>
//!
//! Fires on statement positions (`Seq` elements): a bare call gains a
//! trailing `_rec` read (value-preserving), an `x = …` statement
//! reassigns from `_rec`. Gated on: the receiver chain being
//! `Const.where(HashLit)` with symbol keys and pure-read values (each
//! value is evaluated twice — once querying, once seeding), since the
//! runtime flattens conditions to SQL immediately and can't recover
//! the pairs later. Anything else keeps the runtime
//! `first_or_initialize` (blank-build residue) or fails resolution
//! honestly.
//!
//! Purely shape-directed; runs on the post-analyze hook
//! (`apply_post_analyze_lowerings`) with its siblings so every target
//! consumes the grounded form.

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};

pub fn apply_first_or_create_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let ExprNode::Seq { exprs } = &mut *expr.node else { return };
    for e in exprs {
        // A bare call keeps its value via a trailing `_rec` read; an
        // assign statement rebuilds as inline + `target = _rec` (the
        // whole STATEMENT becomes the Seq — a Seq must never land in
        // the assign's value slot, the emitter renders that broken).
        let (save, reassign, send_expr) = match &*e.node {
            ExprNode::Send { .. } => match claims(e) {
                Some(save) => (save, None, e.clone()),
                None => continue,
            },
            ExprNode::Assign { target, value } => match claims(value) {
                Some(save) => (save, Some(target.clone()), value.clone()),
                None => continue,
            },
            _ => continue,
        };
        let mut stmts = inline(&send_expr, save);
        let span = send_expr.span;
        let rec_read = Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_rec") });
        stmts.push(match reassign {
            None => rec_read,
            Some(target) => Expr::new(span, ExprNode::Assign { target, value: rec_read }),
        });
        *e.node = ExprNode::Seq { exprs: stmts };
        e.ty = None;
    }
}

/// Does this Send match the claimable shape? Returns `Some(save)` —
/// whether the built record saves (`first_or_create`) or stays
/// unsaved (`first_or_initialize`).
fn claims(e: &Expr) -> Option<bool> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    let save = match method.as_str() {
        "first_or_create" => true,
        "first_or_initialize" => false,
        _ => return None,
    };
    if !args.is_empty() {
        return None;
    }
    let ExprNode::Send { recv: Some(model), method: wm, args: wargs, block: None, .. } = &*r.node
    else {
        return None;
    };
    if wm.as_str() != "where" || wargs.len() != 1 || !matches!(&*model.node, ExprNode::Const { .. })
    {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*wargs[0].node else { return None };
    let ok = !entries.is_empty()
        && entries.iter().all(|(k, v)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } })
                && super::case_lambda::is_pure_read(v)
        });
    ok.then_some(save)
}

fn inline(e: &Expr, save: bool) -> Vec<Expr> {
    let span = e.span;
    let ExprNode::Send { recv: Some(r), .. } = &*e.node else { unreachable!() };
    let ExprNode::Send { recv: Some(model), args: wargs, .. } = &*r.node else { unreachable!() };
    let ExprNode::Hash { entries, .. } = &*wargs[0].node else { unreachable!() };

    let rec = |()| Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_rec") });
    let send = |recv: Expr, m: &str| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(m),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };

    let find = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("_rec") },
            value: send(r.clone(), "first"),
        },
    );

    let mut build = vec![Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from("_rec") },
            value: send(model.clone(), "new"),
        },
    )];
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            unreachable!()
        };
        build.push(Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: rec(()), name: name.clone() },
                value: v.clone(),
            },
        ));
    }
    if save {
        build.push(send(rec(()), "save"));
    }

    let guard = Expr::new(
        span,
        ExprNode::If {
            cond: send(rec(()), "nil?"),
            then_branch: Expr::new(span, ExprNode::Seq { exprs: build }),
            else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        },
    );
    vec![find, guard]
}
