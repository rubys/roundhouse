//! A model-body `include` guarded by `begin … rescue NameError … end`.
//!
//! ```ruby
//! class Part < ApplicationRecord
//!   begin
//!     include(Chime)
//!   rescue NameError
//!   end
//! end
//! ```
//!
//! "Include it when the module is there." The body walk keeps the
//! `begin` as one unknown item, and nothing after it reads includes out
//! of a `begin`: the module never reached the class. This pass runs
//! once every file is ingested, so it knows which modules the app
//! defines, and answers what Ruby does: the includes run in order, and
//! the first one naming a module the app does not define raises the
//! `NameError` the rescue swallows. The block is the includes before
//! that one (all of them when every module is defined).
//!
//! Only this exact shape is read: a body of bare `include Const` calls,
//! a rescue clause that catches `NameError` (naming it, `StandardError`
//! or `Exception`, or a bare `rescue`), every rescue body empty (or
//! `nil`), no `else` and no `ensure`. Any other `begin` is left as it
//! was: a rescue that only names `LoadError` does not catch the
//! `NameError`, and a rescue with a body (a fallback include) does
//! something this pass does not model.
//!
//! A constant that names an app **class** is not spliced: Ruby raises
//! `TypeError`. When the rescue catches `TypeError` (`StandardError`,
//! `Exception`, `TypeError`, or a bare `rescue`), the successful module
//! prefix stays. When it only catches `NameError`, the whole `begin` is
//! left alone — splicing would erase an unrescued load failure.
//!
//! A module that still carries a class-side `included` method after
//! ingest (a custom `def self.included`, not the Concern `included do`
//! DSL) is also left alone: `include` can raise from that callback, and
//! the rescue must stay around the call.
//!
//! A Ruby core/stdlib module (`Enumerable`, …) counts as defined even
//! though it is outside the app set. Relative lookup tries the model's
//! own nest (`A::B::Chime` inside `A::B`) then the root; it does **not**
//! invent parent scopes (`class A::B` does not open `A`).

use std::collections::HashSet;

use crate::app::App;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

pub(super) fn splice_guarded_includes(app: &mut App) {
    let known_modules: HashSet<ClassId> = app
        .library_classes
        .iter()
        .filter(|lc| lc.is_module)
        .map(|lc| lc.name.clone())
        .chain(app.concern_model_items.keys().cloned())
        .collect();
    let known_classes: HashSet<ClassId> = app
        .library_classes
        .iter()
        .filter(|lc| !lc.is_module)
        .map(|lc| lc.name.clone())
        .chain(app.models.iter().map(|m| m.name.clone()))
        .collect();
    // Residual `def self.included` after ingest — Concern's `included do`
    // is not a MethodDef. A custom hook can raise NameError under the
    // guard; bare-splicing would let that escape.
    let modules_with_included_hook: HashSet<ClassId> = app
        .library_classes
        .iter()
        .filter(|lc| lc.is_module)
        .filter(|lc| {
            lc.methods.iter().any(|m| {
                m.receiver == MethodReceiver::Class && m.name.as_str() == "included"
            })
        })
        .map(|lc| lc.name.clone())
        .collect();
    for model in &mut app.models {
        let owner = model.name.clone();
        let mut body = Vec::with_capacity(model.body.len());
        for item in std::mem::take(&mut model.body) {
            let ModelBodyItem::Unknown {
                expr,
                leading_comments,
                leading_blank_line,
            } = &item
            else {
                body.push(item);
                continue;
            };
            let Some(guard) = guarded_includes(expr) else {
                body.push(item);
                continue;
            };
            // Walk calls in order. `include A, B` looks both up before
            // including either. Stop rules:
            // - Unknown: keep the module prefix (NameError swallowed).
            // - Class, rescue catches TypeError: keep the module prefix.
            // - Class, rescue does not catch TypeError: leave the begin
            //   (do not erase an unrescued TypeError).
            // - Module with a residual `included` hook: leave the begin.
            let mut prefix: Vec<&Expr> = Vec::new();
            let mut leave_begin = false;
            for include in &guard.includes {
                let lookups: Vec<Lookup> = include_consts(include)
                    .map(|path| lookup(path, &owner, &known_modules, &known_classes))
                    .collect();
                if lookups.iter().any(|l| matches!(l, Lookup::Unknown)) {
                    break;
                }
                if lookups.iter().any(|l| matches!(l, Lookup::Class)) {
                    if !guard.catches_type_error {
                        leave_begin = true;
                    }
                    break;
                }
                if lookups.iter().any(|l| match l {
                    Lookup::Module(id) => modules_with_included_hook.contains(id),
                    _ => false,
                }) {
                    leave_begin = true;
                    break;
                }
                prefix.push(include);
            }
            if leave_begin {
                body.push(item);
                continue;
            }
            let mut comments = leading_comments.clone();
            let mut blank = *leading_blank_line;
            for include in prefix {
                body.push(ModelBodyItem::Unknown {
                    expr: include.clone(),
                    leading_comments: std::mem::take(&mut comments),
                    leading_blank_line: std::mem::take(&mut blank),
                });
            }
        }
        model.body = body;
    }
}

struct Guarded<'a> {
    includes: Vec<&'a Expr>,
    /// Rescue catches `TypeError` (bare, `StandardError`, `Exception`,
    /// or `TypeError` itself).
    catches_type_error: bool,
}

/// The `include` calls of a guarded block, or None when `expr` is not
/// one.
fn guarded_includes(expr: &Expr) -> Option<Guarded<'_>> {
    let ExprNode::BeginRescue {
        body,
        rescues,
        else_branch: None,
        ensure: None,
        implicit: false,
    } = &*expr.node
    else {
        return None;
    };
    // `NameError` or an ancestor; a bare `rescue` is `StandardError`.
    let catches_name_error = rescues.iter().any(|clause| {
        clause.classes.is_empty()
            || clause.classes.iter().any(|class| {
                matches!(&*class.node, ExprNode::Const { path }
                    if path.last().is_some_and(|n| matches!(n.as_str(), "NameError" | "StandardError" | "Exception"))
                        && (path.len() == 1 || (path.len() == 2 && path[0].as_str().is_empty())))
            })
    });
    let catches_type_error = rescues.iter().any(|clause| {
        clause.classes.is_empty()
            || clause.classes.iter().any(|class| {
                matches!(&*class.node, ExprNode::Const { path }
                    if path.last().is_some_and(|n| matches!(n.as_str(), "TypeError" | "StandardError" | "Exception"))
                        && (path.len() == 1 || (path.len() == 2 && path[0].as_str().is_empty())))
            })
    });
    let rescues_do_nothing = rescues.iter().all(|clause| is_empty_body(&clause.body));
    if !catches_name_error || !rescues_do_nothing {
        return None;
    }
    let statements: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let is_include = |e: &&Expr| {
        matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "include"
                && !args.is_empty()
                && args.iter().all(|a| matches!(&*a.node, ExprNode::Const { .. })))
    };
    if statements.is_empty() || !statements.iter().all(is_include) {
        return None;
    }
    Some(Guarded {
        includes: statements,
        catches_type_error,
    })
}

/// A rescue body that does nothing: empty, or a lone `nil`.
fn is_empty_body(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().all(is_empty_body),
        ExprNode::Lit {
            value: crate::expr::Literal::Nil,
        } => true,
        _ => false,
    }
}

fn include_consts(include: &Expr) -> impl Iterator<Item = &[Symbol]> {
    let args: &[Expr] = match &*include.node {
        ExprNode::Send { args, .. } => args,
        _ => &[],
    };
    args.iter().filter_map(|a| match &*a.node {
        ExprNode::Const { path } => Some(path.as_slice()),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lookup {
    /// An includable module the app (or Ruby core) defines.
    Module(ClassId),
    /// An app class — `include` would TypeError.
    Class,
    /// Not in the app set and not a known core module.
    Unknown,
}

/// Const lookup for an include argument under `owner`.
///
/// Absolute paths (`::Chime`) are root-only. Relative paths try the
/// model's own nest first (`A::B::Chime` inside `A::B`), then the root
/// — never parent scopes invented by splitting a qualified class name
/// (`class A::B` does not open `A`).
fn lookup(
    path: &[Symbol],
    owner: &ClassId,
    modules: &HashSet<ClassId>,
    classes: &HashSet<ClassId>,
) -> Lookup {
    let absolute = path.first().is_some_and(|s| s.as_str().is_empty());
    let segments: Vec<&str> = if absolute {
        path.iter().skip(1).map(|s| s.as_str()).collect()
    } else {
        path.iter().map(|s| s.as_str()).collect()
    };
    if segments.is_empty() {
        return Lookup::Unknown;
    }
    let name = segments.join("::");
    if !absolute {
        let scoped_name = format!("{owner}::{name}");
        let scoped_id = ClassId(Symbol::from(scoped_name.as_str()));
        if modules.contains(&scoped_id) {
            return Lookup::Module(scoped_id);
        }
        if classes.contains(&scoped_id) {
            return Lookup::Class;
        }
    }
    let id = ClassId(Symbol::from(name.as_str()));
    if modules.contains(&id) || is_ruby_core_module(&segments) {
        Lookup::Module(id)
    } else if classes.contains(&id) {
        Lookup::Class
    } else {
        Lookup::Unknown
    }
}

/// Core/stdlib modules a guarded `include` may name without the app
/// defining them. Kept small — only names that are modules in Ruby and
/// appear in real Rails apps' optional-include guards.
fn is_ruby_core_module(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["Enumerable"]
            | ["Comparable"]
            | ["Kernel"]
            | ["Forwardable"]
            | ["SingleForwardable"]
            | ["JSON"]
            | ["FileUtils"]
    )
}
