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
//! honest, where a flag one writer never sets would answer wrong. So
//! does any other writer of the ivar: a compound assignment
//! (`@x ||= {}`), a multiple assignment, `instance_variable_set` or
//! `remove_instance_variable` naming it. A class-level
//! `@cache ||= {}` in one method and `return unless defined?(@cache) &&
//! @cache` in another is the shape: the guard would read a flag the
//! compound assignment never set, and always return early.

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
        rewrite_assignments(body, &rewritable, true, true);
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
        ExprNode::OpAssign { target: LValue::Ivar { name: n }, .. } if n == name => false,
        ExprNode::MultiAssign { targets, .. }
            if targets.iter().any(|t| matches!(t, LValue::Ivar { name: n } if n == name)) =>
        {
            false
        }
        ExprNode::Send { method, args, .. }
            if matches!(method.as_str(), "instance_variable_set" | "remove_instance_variable")
                && args.first().is_some_and(|a| names_ivar(a, name)) =>
        {
            false
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

/// `arg` is the ivar's name as a literal (`:@x`, `"@x"`); `name` is
/// stored without the sigil.
fn names_ivar(arg: &Expr, name: &Symbol) -> bool {
    let text = match &*arg.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
        ExprNode::Lit { value: Literal::Str { value } } => value.as_str(),
        _ => return false,
    };
    text.strip_prefix('@') == Some(name.as_str())
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

/// `used`: the statement's value is read (a body's or branch's last
/// expression). Where it is not, the rewrite ends at the flag: a bare
/// trailing `@x` in void context is a CRuby warning ("possibly useless
/// use of a variable"), which a warnings-as-errors boot raises.
fn rewrite_assignments(e: &mut Expr, names: &[Symbol], statement: bool, used: bool) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => {
            let last = exprs.len().saturating_sub(1);
            for (i, x) in exprs.iter_mut().enumerate() {
                rewrite_assignments(x, names, true, used && i == last);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            rewrite_assignments(cond, names, false, true);
            rewrite_assignments(then_branch, names, true, used);
            rewrite_assignments(else_branch, names, true, used);
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } if statement && names.contains(name) => {
            rewrite_assignments(value, names, false, true);
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
            let mut seq = if used {
                let mut read = Expr::new(span, ExprNode::Ivar { name });
                read.ty = ty.clone();
                Expr::new(span, ExprNode::Seq { exprs: vec![assign, set_flag, read] })
            } else {
                Expr::new(span, ExprNode::Seq { exprs: vec![assign, set_flag] })
            };
            seq.ty = if used { ty } else { Some(Ty::Bool) };
            *e = seq;
        }
        _ => {
            e.node.for_each_child_mut(&mut |c| rewrite_assignments(c, names, false, true));
        }
    }
}
