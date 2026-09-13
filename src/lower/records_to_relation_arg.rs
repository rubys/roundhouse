//! A record Array handed to a Relation-typed parameter becomes the
//! Relation over those records.
//!
//! `Rooms::Direct.find_or_create_for(users)` is typed from its one app
//! call site, which passes `User.where(id: …)` — an
//! `ActiveRecord::Relation`, and the body's `users.pluck(:id)` is a
//! SELECT against it. campfire's own tests call the same method with
//! `[ users(:david), users(:kevin) ]`. Ruby does not care: an Array
//! `pluck`s too (activesupport reopens it). A typed emit has to pick,
//! and the analyzer never sees a test body, so nothing widened the
//! parameter — the compiled test handed an `sp_PolyArray *` where an
//! `sp_Relation *` was declared and the file never linked; the CRuby
//! lane ran and raised `undefined method 'pluck' for an instance of
//! Array` (rooms_direct_test, 3 tests, both lanes).
//!
//! The method keeps the app's shape. It is the test's argument that is
//! restated: `[a, b]` at a `Relation[M]` parameter becomes `M.where(id:
//! [a.id, b.id])`, the Relation whose rows are exactly those records.
//! Same set, one query more — which is the cost the app's own caller
//! pays. Widening the parameter instead would make every app method a
//! test calls differently a union at the def, a poly slot on the hot
//! side of the program for the sake of the cold one.
//!
//! Type-directed twice over — the parameter from
//! `App::inferred_method_params`, the elements from the typed body —
//! so it runs after the test lowerer's final typing pass and before
//! `apply_scope_lowering`, which turns the `M.where` it writes into the
//! seeded Relation everything else is.
//!
//! ONLY AN ARRAY LITERAL WHOSE EVERY ELEMENT IS A RECORD OF `M`. A
//! literal of something else at a Relation parameter is a call the app
//! would also have to answer for; an Array-typed VARIABLE is a shape no
//! test in the corpus writes (campfire's four are all literals), and a
//! `map` over it is a different rewrite with its own evidence to earn.

use crate::app::App;
use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::lower::controller_to_library::util::map_expr;
use crate::ty::Ty;

pub fn rewrite_test_classes(lcs: &mut [LibraryClass], app: &App) {
    if app.inferred_method_params.is_empty() {
        return;
    }
    for lc in lcs.iter_mut() {
        for method in &mut lc.methods {
            method.body = map_expr(&method.body, &|e| rewrite_send(e, app));
        }
    }
}

fn rewrite_send(e: &Expr, app: &App) -> Option<Expr> {
    let ExprNode::Send { recv: Some(r), method, args, block, parenthesized } = &*e.node else {
        return None;
    };
    let callee = callee_class(r)?;
    let params = app.inferred_method_params.get(&(callee, method.clone()))?;
    let mut changed = false;
    let rewritten: Vec<Expr> = args
        .iter()
        .enumerate()
        .map(|(i, arg)| match params.get(i) {
            Some(Ty::Relation { of }) => match relation_over(arg, of) {
                Some(rel) => {
                    changed = true;
                    rel
                }
                None => arg.clone(),
            },
            _ => arg.clone(),
        })
        .collect();
    if !changed {
        return None;
    }
    Some(Expr::new(
        e.span,
        ExprNode::Send {
            recv: Some(r.clone()),
            method: method.clone(),
            args: rewritten,
            block: block.clone(),
            parenthesized: *parenthesized,
        },
    ))
}

/// The class a call's parameter table is keyed under: the constant a
/// class-method call names, or the class an instance receiver types
/// to. `collect_send_sites` records both the same way.
fn callee_class(recv: &Expr) -> Option<ClassId> {
    if let ExprNode::Const { path } = &*recv.node {
        let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
        return Some(ClassId(Symbol::from(joined.as_str())));
    }
    match &recv.ty {
        Some(Ty::Class { id, .. }) => Some(id.clone()),
        _ => None,
    }
}

/// `[a, b]`, every element a record of `model` → `model.where(id:
/// [a.id, b.id])`. None for any other argument.
fn relation_over(arg: &Expr, model: &ClassId) -> Option<Expr> {
    let ExprNode::Array { elements, .. } = &*arg.node else { return None };
    if elements.is_empty() {
        return None;
    }
    if !elements.iter().all(|el| is_record_of(el, model)) {
        return None;
    }
    let span = arg.span;
    let ids = Expr::new(
        span,
        ExprNode::Array {
            elements: elements
                .iter()
                .map(|el| {
                    Expr::new(
                        el.span,
                        ExprNode::Send {
                            recv: Some(el.clone()),
                            method: Symbol::from("id"),
                            args: vec![],
                            block: None,
                            parenthesized: false,
                        },
                    )
                })
                .collect(),
            style: Default::default(),
        },
    );
    let key = Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("id") } });
    let conditions = Expr::new(span, ExprNode::Hash { entries: vec![(key, ids)], kwargs: true });
    let path: Vec<Symbol> = model.0.as_str().split("::").map(Symbol::from).collect();
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method: Symbol::from("where"),
            args: vec![conditions],
            block: None,
            parenthesized: true,
        },
    ))
}

fn is_record_of(el: &Expr, model: &ClassId) -> bool {
    matches!(&el.ty, Some(Ty::Class { id, .. }) if id == model)
}
