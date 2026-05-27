//! `mutates_self` propagation — annotate MethodDef with whether the
//! method's body (directly or transitively) writes instance state.
//!
//! Strict-typed targets need to pick the receiver shape at emit time
//! (`&mut self` vs `&self` in Rust; `def` annotation choices in
//! Crystal/Go/Kotlin/Swift). Permissive targets (TS, Ruby) ignore the
//! flag. Centralizing the analysis here means each target reads
//! `m.mutates_self` instead of recomputing the heuristic per emit.
//!
//! Algorithm: two-pass at class level. Seed with methods that have
//! local writes (`@ivar = v`, `self[k] = v`, `self.foo = v`, or
//! `self.foo=(v)` Send-form). Then iterate to fixed point: a method
//! also mutates if it `Send`s to another method on `self` that's in
//! the mutating set. Stops growing when no new method flips.
//!
//! Operates on whole `LibraryClass` Vecs because the transitive
//! closure spans the class — same-class siblings need to see each
//! other's seed flags. Cross-class dispatch isn't followed; a method
//! calling `other_obj.save` isn't marked because the analysis can't
//! tell if `other_obj` aliases `self` or which class's `save`
//! resolves.

use std::collections::HashSet;

use crate::dialect::{LibraryClass, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue};

/// Annotate every instance method's `mutates_self` flag across `classes`.
/// Walks each class's method set independently, seeds the direct-mutator
/// set, then propagates transitively to fixed point.
pub fn propagate(classes: &mut [LibraryClass]) {
    for class in classes.iter_mut() {
        propagate_one(class);
    }
}

/// Single-class transitive pass — exported so callers that work
/// class-by-class (lowerer emit pipelines) can drive it without
/// rebuilding a `&mut [LibraryClass]`.
pub fn propagate_one(class: &mut LibraryClass) {
    let mut mutating: HashSet<String> = class
        .methods
        .iter()
        .filter(|m| matches!(m.receiver, MethodReceiver::Instance))
        .filter(|m| has_local_mutation(&m.body))
        .map(|m| m.name.as_str().to_string())
        .collect();

    loop {
        let before = mutating.len();
        for m in &class.methods {
            if !matches!(m.receiver, MethodReceiver::Instance) {
                continue;
            }
            if mutating.contains(m.name.as_str()) {
                continue;
            }
            if calls_self_method_in(&m.body, &mutating) {
                mutating.insert(m.name.as_str().to_string());
            }
        }
        if mutating.len() == before {
            break;
        }
    }

    for m in class.methods.iter_mut() {
        if matches!(m.receiver, MethodReceiver::Instance)
            && mutating.contains(m.name.as_str())
        {
            m.mutates_self = true;
        }
    }
}

/// True when `body` directly writes instance state. The seed set for
/// the transitive pass.
fn has_local_mutation(body: &Expr) -> bool {
    fn walk(e: &Expr) -> bool {
        match &*e.node {
            ExprNode::Assign { target, .. } => match target {
                LValue::Ivar { .. } => true,
                LValue::Attr { recv, .. } | LValue::Index { recv, .. } => {
                    matches!(&*recv.node, ExprNode::SelfRef | ExprNode::Ivar { .. })
                        || walk(recv)
                }
                LValue::Var { .. } | LValue::Const { .. } => false,
            },
            // `self[k] = v` and `self.foo = v` lower as `Send`s to
            // `[]=` / setter-suffixed methods, not Assign. Same for
            // `@data[k] = v` — direct ivar mutation via index assign
            // (HWIA `set` / `delete`). Treat all as mutation.
            ExprNode::Send { recv: Some(recv), method, .. }
                if matches!(&*recv.node, ExprNode::SelfRef | ExprNode::Ivar { .. })
                    && (method.as_str() == "[]=" || method.as_str().ends_with('='))
                    && !is_comparison_method(method.as_str()) =>
            {
                true
            }
            ExprNode::Seq { exprs } => exprs.iter().any(walk),
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond) || walk(then_branch) || walk(else_branch)
            }
            ExprNode::While { cond, body, .. } => walk(cond) || walk(body),
            ExprNode::Send { recv, args, block, .. } => {
                recv.as_ref().map(|r| walk(r)).unwrap_or(false)
                    || args.iter().any(walk)
                    || block.as_ref().map(|b| walk(b)).unwrap_or(false)
            }
            ExprNode::Return { value } => walk(value),
            // `case scrutinee; when …; body; end` — each arm body can
            // contain `@ivar = …` writes (lowerer-synthesized
            // `set_index` has one arm per column doing
            // `@<col> = value.as(T)`). Without this, set_index
            // misclassifies as non-mutating and emits as `&self`,
            // blowing every Assign LValue::Ivar into E0594.
            ExprNode::Case { scrutinee, arms } => {
                walk(scrutinee) || arms.iter().any(|a| walk(&a.body))
            }
            ExprNode::Cast { value, .. } => walk(value),
            _ => false,
        }
    }
    walk(body)
}

/// Walk `body` looking for `Send { recv: SelfRef, method: M }` where
/// `M` is in `mutating`. `save!` calls `self.save` (mutating) →
/// `save!` mutates too. Only SelfRef receivers count — Ivar-recv calls
/// (`@x.push(y)`) might mutate `@x`'s pointee, but tracking that
/// requires whole-program analysis and the `LValue::Index { recv: Ivar }`
/// case in `has_local_mutation` already covers `@data[k] = v`.
fn calls_self_method_in(body: &Expr, mutating: &HashSet<String>) -> bool {
    fn walk(e: &Expr, mutating: &HashSet<String>) -> bool {
        match &*e.node {
            ExprNode::Send { recv: Some(recv), method, args, block, .. } => {
                if matches!(&*recv.node, ExprNode::SelfRef)
                    && mutating.contains(method.as_str())
                {
                    return true;
                }
                walk(recv, mutating)
                    || args.iter().any(|a| walk(a, mutating))
                    || block.as_ref().map(|b| walk(b, mutating)).unwrap_or(false)
            }
            ExprNode::Send { recv: None, method, args, block, .. } => {
                if mutating.contains(method.as_str()) {
                    return true;
                }
                args.iter().any(|a| walk(a, mutating))
                    || block.as_ref().map(|b| walk(b, mutating)).unwrap_or(false)
            }
            ExprNode::Seq { exprs } => exprs.iter().any(|x| walk(x, mutating)),
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond, mutating)
                    || walk(then_branch, mutating)
                    || walk(else_branch, mutating)
            }
            ExprNode::While { cond, body, .. } => {
                walk(cond, mutating) || walk(body, mutating)
            }
            ExprNode::Return { value } => walk(value, mutating),
            ExprNode::Assign { value, .. } => walk(value, mutating),
            // `case scrutinee; when …; body; end` — each arm body can
            // call mutating sibling methods. The canonical case: the
            // controller lowerer-synthesized `process_action`'s body
            // is a Case dispatching `action_name` → `self.index()` /
            // `self.show()` / `self.create()` etc. The action methods
            // mutate ivars (Articles@articles, ...), so process_action
            // transitively mutates. Without Case traversal, the
            // transitive seed never reaches the action calls and
            // process_action emits as `&self`, blowing every action
            // dispatch with E0596.
            ExprNode::Case { scrutinee, arms } => {
                walk(scrutinee, mutating)
                    || arms.iter().any(|a| walk(&a.body, mutating))
            }
            ExprNode::Cast { value, .. } => walk(value, mutating),
            _ => false,
        }
    }
    walk(body, mutating)
}

/// Trailing-`=` filter: `==`, `!=`, `<=`, `>=` end with `=` but are
/// comparison sends, not setters. The setter heuristic above needs
/// to exclude them.
fn is_comparison_method(m: &str) -> bool {
    matches!(m, "==" | "!=" | "<=" | ">=" | "<=>" | "===" | "=~")
}
