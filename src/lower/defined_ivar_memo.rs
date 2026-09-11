//! `return @x if defined?(@x)` — Rails' memoisation guard for a value
//! that may be nil — lowered to a presence flag the strict targets can
//! carry.
//!
//! CRuby answers `defined?(@x)` from the object: nil until the ivar is
//! first assigned. On a struct-backed target every ivar exists from
//! allocation, so there is nothing to consult — spinel folds the test
//! at compile time (docs/limitations.md, "`defined?(@ivar)` is answered
//! at compile time") and the guard reads the unassigned slot forever:
//! campfire's `Opengraph::Location#parsed_url` answered nil for every
//! URL, so every location was "invalid" and nothing was ever unfurled.
//! The other strict emitters have no `defined?` at all.
//!
//! The rewrite is the one that document prescribes, done once here
//! instead of by hand in each app: `defined?(@x)` becomes a read of
//! `@x_defined`, and every assignment to `@x` in the class sets the
//! flag after the value lands —
//!
//! ```ruby
//! return @x if defined?(@x)      # → return @x if @x_defined
//! @x = compute                   # → @x = compute; @x_defined = true; @x
//! ```
//!
//! The flag is a bool ivar (false — or nil, on the ruby family — until
//! set; either is falsy), so the transformed program is the same
//! program on CRuby and the intended one everywhere else. The value's
//! assignment keeps its position, and the flag is set AFTER it, so a
//! `compute` that raises leaves the object as CRuby would: unassigned.
//!
//! Scope and refusal. The guard is rewritten only when EVERY assignment
//! to that ivar in the class sits in statement position (a `Seq`
//! element or a body's sole expression), where the three-statement
//! replacement is an ordinary sequence. An assignment nested inside an
//! expression the pass cannot expand leaves the `defined?` untouched:
//! a guard that stays `defined?` fails the way it does today, which is
//! honest, where a flag one writer never sets would answer wrong.

use std::collections::BTreeSet;

use crate::app::App;
use crate::dialect::{ControllerBodyItem, ModelBodyItem};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_defined_ivar_memo_lowering(app: &mut App) {
    for model in &mut app.models {
        let mut bodies: Vec<&mut Expr> = Vec::new();
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => bodies.push(&mut method.body),
                ModelBodyItem::Scope { scope, .. } => bodies.push(&mut scope.body),
                _ => {}
            }
        }
        rewrite_class(bodies);
    }
    for lc in &mut app.library_classes {
        let bodies: Vec<&mut Expr> = lc.methods.iter_mut().map(|m| &mut m.body).collect();
        rewrite_class(bodies);
    }
    if let Some(lc) = &mut app.rails_application {
        let bodies: Vec<&mut Expr> = lc.methods.iter_mut().map(|m| &mut m.body).collect();
        rewrite_class(bodies);
    }
    for controller in &mut app.controllers {
        let mut bodies: Vec<&mut Expr> = Vec::new();
        for item in &mut controller.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                bodies.push(&mut action.body);
            }
        }
        rewrite_class(bodies);
    }
}

fn rewrite_class(mut bodies: Vec<&mut Expr>) {
    let mut guarded: BTreeSet<Symbol> = BTreeSet::new();
    for body in bodies.iter() {
        collect_guarded(body, &mut guarded);
    }
    if guarded.is_empty() {
        return;
    }
    // An ivar whose every assignment can take the flag.
    let rewritable: Vec<Symbol> = guarded
        .iter()
        .filter(|name| {
            bodies.iter().all(|body| assignments_in_statement_position(body, name, true))
        })
        .cloned()
        .collect();
    if rewritable.is_empty() {
        return;
    }
    for body in bodies.iter_mut() {
        rewrite_guards(body, &rewritable);
        rewrite_assignments(body, &rewritable, true);
    }
}

fn flag_name(name: &Symbol) -> Symbol {
    Symbol::from(format!("{}_defined", name.as_str()).as_str())
}

fn collect_guarded(e: &Expr, out: &mut BTreeSet<Symbol>) {
    if let ExprNode::Send { recv: None, method, args, .. } = &*e.node {
        if method.as_str() == "defined?" && args.len() == 1 {
            if let ExprNode::Ivar { name } = &*args[0].node {
                out.insert(name.clone());
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_guarded(c, out));
}

/// Every `@name = …` in `e` is a statement — `statement` says whether
/// `e` itself is one. A `Seq`'s elements are; so is a body's sole
/// expression; a branch of an `If` is; an assignment anywhere else is
/// a value some emitter would have to expand in place.
fn assignments_in_statement_position(e: &Expr, name: &Symbol, statement: bool) -> bool {
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name: n }, value } => {
            if n == name && !statement {
                return false;
            }
            assignments_in_statement_position(value, name, false)
        }
        ExprNode::Seq { exprs } => {
            exprs.iter().all(|x| assignments_in_statement_position(x, name, true))
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            assignments_in_statement_position(cond, name, false)
                && assignments_in_statement_position(then_branch, name, true)
                && assignments_in_statement_position(else_branch, name, true)
        }
        _ => {
            let mut ok = true;
            e.node.for_each_child(&mut |c| {
                if ok && !assignments_in_statement_position(c, name, false) {
                    ok = false;
                }
            });
            ok
        }
    }
}

fn rewrite_guards(e: &mut Expr, names: &[Symbol]) {
    e.node.for_each_child_mut(&mut |c| rewrite_guards(c, names));
    let span = e.span;
    let replacement = match &*e.node {
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "defined?" && args.len() == 1 =>
        {
            match &*args[0].node {
                ExprNode::Ivar { name } if names.contains(name) => Some(flag_name(name)),
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(flag) = replacement {
        let mut read = Expr::new(span, ExprNode::Ivar { name: flag });
        read.ty = Some(Ty::Union { variants: vec![Ty::Bool, Ty::Nil] });
        *e = read;
    }
}

fn rewrite_assignments(e: &mut Expr, names: &[Symbol], statement: bool) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => {
            for x in exprs.iter_mut() {
                rewrite_assignments(x, names, true);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            rewrite_assignments(cond, names, false);
            rewrite_assignments(then_branch, names, true);
            rewrite_assignments(else_branch, names, true);
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } if statement && names.contains(name) => {
            rewrite_assignments(value, names, false);
            let span = e.span;
            let name = name.clone();
            let ty = e.ty.clone();
            let assign = std::mem::replace(e, Expr::new(span, ExprNode::Lit { value: Literal::Nil }));
            let mut flag_true = Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: true } });
            flag_true.ty = Some(Ty::Bool);
            let mut set_flag = Expr::new(
                span,
                ExprNode::Assign { target: LValue::Ivar { name: flag_name(&name) }, value: flag_true },
            );
            set_flag.ty = Some(Ty::Bool);
            let mut read = Expr::new(span, ExprNode::Ivar { name });
            read.ty = ty.clone();
            let mut seq = Expr::new(span, ExprNode::Seq { exprs: vec![assign, set_flag, read] });
            seq.ty = ty;
            *e = seq;
        }
        _ => {
            e.node.for_each_child_mut(&mut |c| rewrite_assignments(c, names, false));
        }
    }
}
