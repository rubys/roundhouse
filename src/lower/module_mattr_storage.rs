//! A module's `mattr_*` storage as a module-level instance variable.
//!
//! Ingest expands `mattr_accessor :x` into `def self.x; @@x; end`, its
//! writer, and the seed `@@x = nil unless defined?(@@x)`
//! (`ingest::library_class::mattr_nil_seed`), as Rails stores it. Spinel
//! has no class variables, and refuses the reads, the writes and the
//! seed's `defined?` (`project::report_native_ruby_syntax`). Campfire's
//! script-source memo (`InlineScriptHashes.last_script_sources`,
//! once-campfire#370) is one.
//!
//! In a MODULE, and when nothing outside the module names `@@x`, the class
//! variable and the module's own instance variable behave alike: the
//! module's singleton methods are the only code that reads or writes it,
//! and a module is not subclassed, so the hierarchy sharing a class
//! variable gives a class is never in play. Those are the cases rewritten,
//! on every target: `@@x` becomes `@x`, and the seed, which runs once as
//! the module loads, becomes the plain assignment. A class's `cattr_*`
//! (shared with its subclasses) and a module whose `@@x` another body
//! reads are left as they are.

use std::collections::HashSet;

use crate::app::App;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;

pub fn apply_module_mattr_storage(app: &mut App) {
    // Every `@@name` each body names, by the module that owns the body.
    let mut outside: HashSet<Symbol> = HashSet::new();
    let module_names: HashSet<crate::ident::ClassId> = app
        .library_classes
        .iter()
        .filter(|lc| lc.is_module)
        .map(|lc| lc.name.clone())
        .collect();
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        let own = owner.is_some_and(|o| module_names.contains(o));
        if !own {
            collect_cvars(body, &mut outside);
        }
    });

    for lc in app.library_classes.iter_mut().filter(|lc| lc.is_module) {
        let mut names: HashSet<Symbol> = HashSet::new();
        for method in &lc.methods {
            collect_cvars(&method.body, &mut names);
        }
        for init in &lc.class_ivar_initializers {
            collect_cvars(init, &mut names);
        }
        // The module's own instance variables, so a rewrite cannot merge a
        // class variable into an instance variable of the same name.
        let mut ivars: HashSet<Symbol> = HashSet::new();
        for method in &lc.methods {
            collect_ivars(&method.body, &mut ivars);
        }
        for init in &lc.class_ivar_initializers {
            collect_ivars(init, &mut ivars);
        }
        // Not one an instance method reads: that method runs on whoever includes or extends the module, where `@x` is the other object's.
        let mut shared: HashSet<Symbol> = HashSet::new();
        for method in lc.methods.iter().filter(|m| m.receiver == crate::dialect::MethodReceiver::Instance) {
            collect_cvars(&method.body, &mut shared);
        }
        let movable: HashSet<Symbol> = names
            .into_iter()
            .filter(|cvar| !outside.contains(cvar) && !shared.contains(cvar) && !ivars.contains(&ivar_name(cvar)))
            .collect();
        if movable.is_empty() {
            continue;
        }
        for method in &mut lc.methods {
            rewrite(&mut method.body, &movable);
        }
        for init in &mut lc.class_ivar_initializers {
            rewrite(init, &movable);
        }
    }
}

fn ivar_name(cvar: &Symbol) -> Symbol {
    Symbol::from(cvar.as_str().trim_start_matches("@@"))
}

fn is_cvar(name: &Symbol) -> bool {
    name.as_str().starts_with("@@")
}

fn collect_cvars(expr: &Expr, out: &mut HashSet<Symbol>) {
    match &*expr.node {
        ExprNode::Var { name, .. } if is_cvar(name) => {
            out.insert(name.clone());
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, .. }
        | ExprNode::OpAssign { target: LValue::Var { name, .. }, .. }
            if is_cvar(name) =>
        {
            out.insert(name.clone());
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |child| collect_cvars(child, out));
}

fn collect_ivars(expr: &Expr, out: &mut HashSet<Symbol>) {
    match &*expr.node {
        ExprNode::Ivar { name } => {
            out.insert(name.clone());
        }
        ExprNode::Assign { target: LValue::Ivar { name }, .. }
        | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } => {
            out.insert(name.clone());
        }
        _ => {}
    }
    expr.node.for_each_child(&mut |child| collect_ivars(child, out));
}

fn rewrite(expr: &mut Expr, movable: &HashSet<Symbol>) {
    // The seed first: `if defined?(@@x) then nil else @@x = v end` runs once
    // as the module loads, when `@@x` is never yet defined.
    if let Some(assign) = unguarded_seed(expr, movable) {
        *expr = assign;
    }
    expr.node.for_each_child_mut(&mut |child| rewrite(child, movable));
    match &mut *expr.node {
        ExprNode::Var { name, .. } if movable.contains(name) => {
            let ivar = ivar_name(name);
            *expr.node = ExprNode::Ivar { name: ivar };
        }
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
            if let LValue::Var { name, .. } = target {
                if movable.contains(name) {
                    *target = LValue::Ivar { name: ivar_name(name) };
                }
            }
        }
        _ => {}
    }
}

fn unguarded_seed(expr: &Expr, movable: &HashSet<Symbol>) -> Option<Expr> {
    let ExprNode::If { cond, then_branch, else_branch } = &*expr.node else {
        return None;
    };
    let ExprNode::Defined { operand } = &*cond.node else {
        return None;
    };
    let ExprNode::Var { name: guarded, .. } = &*operand.node else {
        return None;
    };
    if !movable.contains(guarded) || !matches!(&*then_branch.node, ExprNode::Lit { value: Literal::Nil }) {
        return None;
    }
    match &*else_branch.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, .. } if name == guarded => Some(else_branch.clone()),
        _ => None,
    }
}
