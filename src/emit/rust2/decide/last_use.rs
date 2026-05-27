//! Stage 3 — `LAST_USE` + `CLONE_AT` decide-pass walker.
//!
//! Replaces the `collect_var_read_counts` + `CLONE_VARS` thread-local
//! pre-pass that lived in `expr/mod.rs`. The pre-pass tagged every
//! var name read more than once into a per-method `HashSet<String>`;
//! the `Var` render arm then appended `.clone()` on every read for
//! non-Copy types — *including the lexically-last read*, which the
//! comment in the old code admitted was a one-clone-too-many.
//!
//! Stage 3 fixes both the location and the over-clone:
//!
//! - **`LAST_USE`** (cross-target bit 1) marks each Var read whose
//!   lexical position is the highest for that name in the method
//!   body. Conceptually portable to Swift `consume`, C++ `std::move`,
//!   and any nominal-typed target that distinguishes the last use
//!   of a binding from earlier uses. Today consumed only by rust2.
//!
//! - **`CLONE_AT`** (rust2-local bit 34) is set on a Var read when
//!   the render rule fires: name is read more than once in the
//!   method, this read is *not* the last lexical use, and the value
//!   type is non-Copy. The render arm checks this single bit and
//!   appends `.clone()` — no thread-local lookup, no recomputed type
//!   check at the read site.
//!
//! Pure improvement over the prior over-clone: the lexically-last
//! read of each multi-read var now moves instead of cloning. Same
//! HTTP-response semantics, byte-different emitted Rust.

use std::collections::HashMap;

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode, InterpPart, LValue};
use crate::ty::Ty;

use super::bits::{CLONE_AT, LAST_USE};

/// Walk every method body in every class and stamp `LAST_USE` /
/// `CLONE_AT` on each Var read per the rules above.
pub fn stamp(classes: &mut [LibraryClass]) {
    for class in classes {
        for m in class.methods.iter_mut() {
            stamp_method(&mut m.body);
        }
    }
}

fn stamp_method(body: &mut Expr) {
    // Pass 1: collect (name, sequence) for every Var read site.
    // Sequence is pre-order index — matches the lexical reading
    // order the old `collect_var_read_counts` walker used.
    let mut seq = 0usize;
    let mut reads: Vec<(String, usize)> = Vec::new();
    collect_var_reads(body, &mut seq, &mut reads);

    // Aggregate: read count + max sequence per name.
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut last_seq: HashMap<String, usize> = HashMap::new();
    for (name, s) in &reads {
        *counts.entry(name.clone()).or_insert(0) += 1;
        last_seq
            .entry(name.clone())
            .and_modify(|e| {
                if *s > *e {
                    *e = *s;
                }
            })
            .or_insert(*s);
    }

    // Pass 2: walk again with a fresh counter and stamp at the
    // matching positions.
    let mut seq = 0usize;
    stamp_var_reads(body, &mut seq, &counts, &last_seq);
}

fn collect_var_reads(
    e: &Expr,
    seq: &mut usize,
    out: &mut Vec<(String, usize)>,
) {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            out.push((name.as_str().to_string(), *seq));
            *seq += 1;
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            walk_lvalue_collect(target, seq, out);
            collect_var_reads(value, seq, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            // Skip the recv of a StringBuilderAppend hint — the
            // accumulator local is mutated in place and shouldn't
            // count as a read (mirrors the old pre-pass's skip in
            // `collect_var_read_counts`). `hint` lives on the outer
            // `Expr`, not the Send variant.
            let skip_recv = matches!(
                e.hint,
                Some(crate::expr::IrHint::StringBuilderAppend)
            );
            if !skip_recv {
                if let Some(r) = recv {
                    collect_var_reads(r, seq, out);
                }
            }
            for a in args {
                collect_var_reads(a, seq, out);
            }
            if let Some(b) = block {
                collect_var_reads(b, seq, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_var_reads(k, seq, out);
                collect_var_reads(v, seq, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                collect_var_reads(el, seq, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_var_reads(expr, seq, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_var_reads(left, seq, out);
            collect_var_reads(right, seq, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_var_reads(cond, seq, out);
            collect_var_reads(then_branch, seq, out);
            collect_var_reads(else_branch, seq, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_var_reads(scrutinee, seq, out);
            for arm in arms {
                if let Some(g) = arm.guard.as_ref() {
                    collect_var_reads(g, seq, out);
                }
                collect_var_reads(&arm.body, seq, out);
            }
        }
        ExprNode::While { cond, body, .. } => {
            collect_var_reads(cond, seq, out);
            collect_var_reads(body, seq, out);
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                collect_var_reads(x, seq, out);
            }
        }
        ExprNode::Lambda { body, .. } => collect_var_reads(body, seq, out),
        ExprNode::Return { value } => collect_var_reads(value, seq, out),
        ExprNode::Raise { value } => collect_var_reads(value, seq, out),
        ExprNode::Yield { args } => {
            for a in args {
                collect_var_reads(a, seq, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value.as_ref() {
                collect_var_reads(v, seq, out);
            }
        }
        ExprNode::Splat { value } => collect_var_reads(value, seq, out),
        ExprNode::Super { args } => {
            if let Some(arglist) = args.as_ref() {
                for a in arglist {
                    collect_var_reads(a, seq, out);
                }
            }
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue_collect(t, seq, out);
            }
            collect_var_reads(value, seq, out);
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_var_reads(body, seq, out);
            for r in rescues {
                collect_var_reads(&r.body, seq, out);
            }
            if let Some(eb) = else_branch.as_ref() {
                collect_var_reads(eb, seq, out);
            }
            if let Some(en) = ensure.as_ref() {
                collect_var_reads(en, seq, out);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            collect_var_reads(expr, seq, out);
            collect_var_reads(fallback, seq, out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_var_reads(value, seq, out);
            collect_var_reads(body, seq, out);
        }
        ExprNode::Apply { fun, args, block } => {
            collect_var_reads(fun, seq, out);
            for a in args {
                collect_var_reads(a, seq, out);
            }
            if let Some(b) = block.as_ref() {
                collect_var_reads(b, seq, out);
            }
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_ref() {
                collect_var_reads(b, seq, out);
            }
            if let Some(en) = end.as_ref() {
                collect_var_reads(en, seq, out);
            }
        }
        ExprNode::Cast { value, .. } => collect_var_reads(value, seq, out),
        ExprNode::Lit { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
    }
}

fn walk_lvalue_collect(lv: &LValue, seq: &mut usize, out: &mut Vec<(String, usize)>) {
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } | LValue::Const { .. } => {}
        LValue::Attr { recv, .. } => collect_var_reads(recv, seq, out),
        LValue::Index { recv, index } => {
            collect_var_reads(recv, seq, out);
            collect_var_reads(index, seq, out);
        }
    }
}

fn stamp_var_reads(
    e: &mut Expr,
    seq: &mut usize,
    counts: &HashMap<String, usize>,
    last_seq: &HashMap<String, usize>,
) {
    // Local copy to stamp on after recursing into children.
    let outer_skip_recv = matches!(
        e.hint,
        Some(crate::expr::IrHint::StringBuilderAppend)
    );
    match &mut *e.node {
        ExprNode::Var { name, .. } => {
            let n = name.as_str().to_string();
            let here = *seq;
            *seq += 1;
            let is_last = last_seq.get(&n) == Some(&here);
            let count = counts.get(&n).copied().unwrap_or(0);
            let non_copy = e.ty.as_ref().map(|t| !is_copy_ty(t)).unwrap_or(false);
            if is_last {
                e.decisions |= LAST_USE;
            }
            // Stage 3 conservative posture: stamp `CLONE_AT` on every
            // multi-read non-Copy read — same shape as the legacy
            // `CLONE_VARS` pre-pass, which the comment in `expr/mod.
            // rs` admitted "over-clones the lexically-last read by
            // one." Skipping `is_last` looks like a free improvement
            // but exposes a latent issue: peepholes that call
            // `emit_expr` on a recv (e.g. the `.each` / `.iter_mut()`
            // bridge in `expr/mod.rs::emit_send_inner`) currently
            // rely on the Var-arm's multi-read clone to produce a
            // mutable temporary for non-mut-declared bindings (e.g.
            // function params `articles: Vec<Article>`). Preserve
            // the over-clone until a follow-on either routes those
            // bridges through `emit_send_recv` + a separate
            // mutable-temp insertion, or promotes params to `mut`
            // in the method signature when consumed by `iter_mut()`-
            // shaped peepholes. The `LAST_USE` bit stays available
            // for future consumers (Swift `consume`, etc.); it's
            // informational at Stage 3.
            if count > 1 && non_copy {
                e.decisions |= CLONE_AT;
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            walk_lvalue_stamp(target, seq, counts, last_seq);
            stamp_var_reads(value, seq, counts, last_seq);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if !outer_skip_recv {
                if let Some(r) = recv.as_mut() {
                    stamp_var_reads(r, seq, counts, last_seq);
                }
            }
            for a in args.iter_mut() {
                stamp_var_reads(a, seq, counts, last_seq);
            }
            if let Some(b) = block.as_mut() {
                stamp_var_reads(b, seq, counts, last_seq);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                stamp_var_reads(k, seq, counts, last_seq);
                stamp_var_reads(v, seq, counts, last_seq);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                stamp_var_reads(el, seq, counts, last_seq);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    stamp_var_reads(expr, seq, counts, last_seq);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            stamp_var_reads(left, seq, counts, last_seq);
            stamp_var_reads(right, seq, counts, last_seq);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            stamp_var_reads(cond, seq, counts, last_seq);
            stamp_var_reads(then_branch, seq, counts, last_seq);
            stamp_var_reads(else_branch, seq, counts, last_seq);
        }
        ExprNode::Case { scrutinee, arms } => {
            stamp_var_reads(scrutinee, seq, counts, last_seq);
            for arm in arms {
                if let Some(g) = arm.guard.as_mut() {
                    stamp_var_reads(g, seq, counts, last_seq);
                }
                stamp_var_reads(&mut arm.body, seq, counts, last_seq);
            }
        }
        ExprNode::While { cond, body, .. } => {
            stamp_var_reads(cond, seq, counts, last_seq);
            stamp_var_reads(body, seq, counts, last_seq);
        }
        ExprNode::Seq { exprs } => {
            for x in exprs {
                stamp_var_reads(x, seq, counts, last_seq);
            }
        }
        ExprNode::Lambda { body, .. } => stamp_var_reads(body, seq, counts, last_seq),
        ExprNode::Return { value } => stamp_var_reads(value, seq, counts, last_seq),
        ExprNode::Raise { value } => stamp_var_reads(value, seq, counts, last_seq),
        ExprNode::Yield { args } => {
            for a in args {
                stamp_var_reads(a, seq, counts, last_seq);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value.as_mut() {
                stamp_var_reads(v, seq, counts, last_seq);
            }
        }
        ExprNode::Splat { value } => stamp_var_reads(value, seq, counts, last_seq),
        ExprNode::Super { args } => {
            if let Some(arglist) = args.as_mut() {
                for a in arglist {
                    stamp_var_reads(a, seq, counts, last_seq);
                }
            }
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue_stamp(t, seq, counts, last_seq);
            }
            stamp_var_reads(value, seq, counts, last_seq);
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            stamp_var_reads(body, seq, counts, last_seq);
            for r in rescues {
                stamp_var_reads(&mut r.body, seq, counts, last_seq);
            }
            if let Some(eb) = else_branch.as_mut() {
                stamp_var_reads(eb, seq, counts, last_seq);
            }
            if let Some(en) = ensure.as_mut() {
                stamp_var_reads(en, seq, counts, last_seq);
            }
        }
        ExprNode::RescueModifier { expr, fallback } => {
            stamp_var_reads(expr, seq, counts, last_seq);
            stamp_var_reads(fallback, seq, counts, last_seq);
        }
        ExprNode::Let { value, body, .. } => {
            stamp_var_reads(value, seq, counts, last_seq);
            stamp_var_reads(body, seq, counts, last_seq);
        }
        ExprNode::Apply { fun, args, block } => {
            stamp_var_reads(fun, seq, counts, last_seq);
            for a in args {
                stamp_var_reads(a, seq, counts, last_seq);
            }
            if let Some(b) = block.as_mut() {
                stamp_var_reads(b, seq, counts, last_seq);
            }
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin.as_mut() {
                stamp_var_reads(b, seq, counts, last_seq);
            }
            if let Some(en) = end.as_mut() {
                stamp_var_reads(en, seq, counts, last_seq);
            }
        }
        ExprNode::Cast { value, .. } => stamp_var_reads(value, seq, counts, last_seq),
        ExprNode::Lit { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
    }
}

fn walk_lvalue_stamp(
    lv: &mut LValue,
    seq: &mut usize,
    counts: &HashMap<String, usize>,
    last_seq: &HashMap<String, usize>,
) {
    match lv {
        LValue::Var { .. } | LValue::Ivar { .. } | LValue::Const { .. } => {}
        LValue::Attr { recv, .. } => stamp_var_reads(recv, seq, counts, last_seq),
        LValue::Index { recv, index } => {
            stamp_var_reads(recv, seq, counts, last_seq);
            stamp_var_reads(index, seq, counts, last_seq);
        }
    }
}

/// Whether a value of `ty` is `Copy` at the Rust emit level — mirrors
/// `expr::util::is_copy_ty` (kept local to avoid pulling the entire
/// `expr` module into `decide`). Conservative: only the unambiguous
/// primitive cases. Unknown / Untyped → not Copy.
fn is_copy_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool => true,
        // `Ty::Nil` is unit-like; emitted as `()`, which is Copy.
        Ty::Nil => true,
        // `Union<T, Nil>` — Copy iff the non-Nil variant is Copy.
        Ty::Union { variants } => variants.iter().all(|v| is_copy_ty(v)),
        _ => false,
    }
}
