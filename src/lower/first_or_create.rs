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
use crate::ident::{ClassId, Symbol, VarId};
use crate::ty::Ty;

pub fn apply_first_or_create_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut |body| {
        // A one-statement method body is not a Seq, so the statement
        // walk below never saw it — lobsters'
        // `def find_or_initialize_domain; @domain = Domain
        // .find_or_initialize_by(…); end`. Give it one to stand in.
        if is_claimable_stmt(body) {
            let stmt = body.clone();
            *body = Expr::new(stmt.span, ExprNode::Seq { exprs: vec![stmt] });
        }
        rewrite(body)
    });
}

fn is_claimable_stmt(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { .. } => claims(&as_where_first(e)).is_some(),
        ExprNode::Assign { value, .. } => claims(&as_where_first(value)).is_some(),
        _ => false,
    }
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
            ExprNode::Send { .. } => {
                let e = as_where_first(e);
                match claims(&e) {
                    Some(save) => (save, None, e),
                    None => continue,
                }
            }
            ExprNode::Assign { target, value } => {
                let value = as_where_first(value);
                match claims(&value) {
                    Some(save) => (save, Some(target.clone()), value),
                    None => continue,
                }
            }
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

/// `Model.find_or_initialize_by(h)` IS `Model.where(h).first_or_initialize`
/// (and `find_or_create_by` the `_create` twin) — Rails defines them
/// that way. Restated in that shape so one inline serves all four;
/// anything else comes back unchanged. Const receivers only: an
/// association receiver keeps the runtime `Relation#find_or_create_by`,
/// whose scope merge is the point (campfire's `user.searches`).
fn as_where_first(e: &Expr) -> Expr {
    let ExprNode::Send { recv: Some(model), method, args, block: None, .. } = &*e.node else {
        return e.clone();
    };
    let first_or = match method.as_str() {
        "find_or_initialize_by" => "first_or_initialize",
        "find_or_create_by" => "first_or_create",
        _ => return e.clone(),
    };
    if args.len() != 1 || !matches!(&*model.node, ExprNode::Const { .. }) {
        return e.clone();
    }
    let mut where_send = Expr::new(
        e.span,
        ExprNode::Send {
            recv: Some(model.clone()),
            method: Symbol::from("where"),
            args: args.clone(),
            block: None,
            parenthesized: true,
        },
    );
    if let ExprNode::Const { path } = &*model.node {
        where_send.ty = Some(Ty::Relation {
            of: ClassId(Symbol::from(
                path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
            )),
        });
    }
    Expr::new(
        e.span,
        ExprNode::Send {
            recv: Some(where_send),
            method: Symbol::from(first_or),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// A condition value is evaluated twice (query, then seed), so it must
/// be a pure read — or a literal-keyed index on one, the shape a
/// controller's `params[:id]` takes.
fn seedable(v: &Expr) -> bool {
    if super::case_lambda::is_pure_read(v) {
        return true;
    }
    matches!(
        &*v.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "[]"
                && args.len() == 1
                && matches!(&*args[0].node, ExprNode::Lit { .. })
                && super::case_lambda::is_pure_read(r)
    )
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
                && seedable(v)
        });
    ok.then_some(save)
}

fn inline(e: &Expr, save: bool) -> Vec<Expr> {
    let span = e.span;
    let ExprNode::Send { recv: Some(r), .. } = &*e.node else { unreachable!() };
    let ExprNode::Send { recv: Some(model), args: wargs, .. } = &*r.node else { unreachable!() };
    let ExprNode::Hash { entries, .. } = &*wargs[0].node else { unreachable!() };

    // This runs after analysis, so the nodes it builds are typed here
    // or not at all — and an untyped send on a typed receiver is what
    // the diagnostics walk reports as a dispatch failure (`no known
    // method \`new\` on ReadRibbon`). The model is known by construction.
    let ExprNode::Const { path } = &*model.node else { unreachable!() };
    let record = Ty::Class {
        id: ClassId(Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        )),
        args: vec![],
    };
    let typed = |mut e: Expr, ty: Ty| {
        e.ty = Some(ty);
        e
    };
    let rec = |()| {
        typed(Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from("_rec") }), record.clone())
    };
    let send = |recv: Expr, m: &str| {
        let ty = match m {
            "first" => Ty::Union { variants: vec![record.clone(), Ty::Nil] },
            "new" => record.clone(),
            _ => Ty::Bool,
        };
        typed(
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(recv),
                    method: Symbol::from(m),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            ty,
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
