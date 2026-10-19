//! Which locals a block-scoped target must declare at the method top.
//!
//! Kotlin and Swift scope a declaration to the branch, loop or closure
//! it appears in. A Ruby local first assigned inside one of those and
//! assigned again elsewhere therefore needs one typed `var` at the top
//! of the method, with every write bare.
//!
//! A block is the exception, because Ruby scopes it the same way: a name
//! a block assigns that is not already a local outside it is local to
//! that block. Two sibling `each` blocks that both assign `controller`
//! hold two unrelated locals, and neither is hoisted. The block's own
//! declaration is the emitter's to scope (see each `emit_lambda`).

use std::collections::BTreeMap;

use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

/// One hoisted local: its Ruby name, and the type of each assignment's
/// value in source order (`None` for a `nil` or untyped value). Each
/// emitter picks the declaration type its own way from `tys`.
pub struct Hoisted {
    pub name: String,
    pub tys: Vec<Option<Ty>>,
}

struct Seen {
    depth: usize,
    tys: Vec<Option<Ty>>,
}

/// Locals first assigned at a nested depth and assigned more than once,
/// sorted by name.
pub fn hoisted_locals(body: &Expr) -> Vec<Hoisted> {
    let mut info: BTreeMap<String, Seen> = BTreeMap::new();
    walk(body, 0, &mut info);
    info.into_iter()
        .filter(|(_, s)| s.depth > 0 && s.tys.len() > 1)
        .map(|(name, s)| Hoisted { name, tys: s.tys })
        .collect()
}

fn walk(e: &Expr, depth: usize, info: &mut BTreeMap<String, Seen>) {
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*e.node {
        let ty = value.ty.clone().filter(|t| !matches!(t, Ty::Nil));
        info.entry(name.as_str().to_string())
            .or_insert(Seen { depth, tys: Vec::new() })
            .tys
            .push(ty);
    }
    // Branch, loop and block bodies are one level deeper; a `Seq` and a
    // condition stay at the current depth.
    match &*e.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, depth, info);
            walk(then_branch, depth + 1, info);
            walk(else_branch, depth + 1, info);
        }
        ExprNode::While { cond, body, .. } => {
            walk(cond, depth, info);
            walk(body, depth + 1, info);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk(scrutinee, depth, info);
            for a in arms {
                walk(&a.body, depth + 1, info);
            }
        }
        ExprNode::Lambda { body, .. } => {
            let mut inner: BTreeMap<String, Seen> = BTreeMap::new();
            walk(body, depth + 1, &mut inner);
            for (name, seen) in inner {
                match info.get_mut(&name) {
                    // A local of the enclosing scope: the block writes it.
                    Some(outer) => outer.tys.extend(seen.tys),
                    // The block's own local. Reassigned inside the block,
                    // it hoists as it did before; assigned once, the
                    // block declares it.
                    None if seen.tys.len() > 1 => {
                        info.insert(name, seen);
                    }
                    None => {}
                }
            }
        }
        ExprNode::Seq { exprs } => {
            for c in exprs {
                walk(c, depth, info);
            }
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            walk(value, depth, info)
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, depth, info);
            }
            for a in args {
                walk(a, depth, info);
            }
            if let Some(b) = block {
                walk(b, depth, info);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, depth, info);
            walk(right, depth, info);
        }
        ExprNode::Return { value } | ExprNode::Raise { value } => walk(value, depth, info),
        _ => {}
    }
}
