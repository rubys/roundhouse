//! A lambda stored in a local has no receiver to take parameter types
//! from, the way a block does. The `.call` / `[]` sites later in the
//! same body are that evidence: `double = ->(n) { n * 2 }; double.call(4)`
//! types `n` as Integer.

use std::collections::HashMap;

use crate::dialect::Param as RubyParam;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

use super::{union_of, Ctx};

/// Parameter types learned from call sites, aligned with the lambda's
/// required positionals. `None` is a hole (an argument this pass could
/// not type); keywords are required keyword parameters only.
#[derive(Clone, Debug)]
pub(super) struct Seeds {
    pub positional: Vec<Option<Ty>>,
    pub keywords: Vec<(Symbol, Ty)>,
}

#[derive(Clone)]
pub(super) struct Stored {
    pub index: usize,
    pub positional: Vec<Option<Ty>>,
    pub keywords: Vec<(Symbol, Ty)>,
    keyword_names: Vec<Symbol>,
    pub ctx: Ctx,
}

pub(super) struct Evidence {
    /// `None` when a splat makes positions unknowable.
    positional: Option<Vec<Option<Ty>>>,
    keywords: Vec<(Symbol, Ty)>,
}

pub(super) fn stored_from(index: usize, value: &Expr, ctx: &Ctx) -> Option<Stored> {
    let ExprNode::Lambda { params, extra_params, .. } = &*value.node else {
        return None;
    };
    let keyword_names: Vec<Symbol> = extra_params
        .iter()
        .filter(|p| required_keyword(p))
        .map(|p| p.name.clone())
        .collect();
    if params.is_empty() && keyword_names.is_empty() {
        return None;
    }
    Some(Stored {
        index,
        positional: Vec::new(),
        keywords: Vec::new(),
        keyword_names,
        ctx: ctx.clone(),
    })
}

pub(super) fn required_keyword(p: &RubyParam) -> bool {
    p.keyword && !p.rest && p.default.is_none() && !p.name.as_str().is_empty()
}

impl Seeds {
    /// Bind the positional parameters this evidence typed. A hole stays unbound.
    pub(super) fn bind_positionals(&self, inner: &mut Ctx, params: &[Symbol]) {
        for (name, ty) in params.iter().zip(self.positional.iter()) {
            let Some(ty) = ty else { continue };
            inner.class_objects.remove(name);
            inner.query_class_objects.remove(name);
            inner.local_bindings.insert(name.clone(), ty.clone());
        }
    }

    /// A required keyword takes its type from a `.call` site. Optional
    /// keywords keep the default's type, which the caller already bound.
    pub(super) fn bind_keywords(&self, inner: &mut Ctx, extra: &[RubyParam]) {
        for (name, ty) in &self.keywords {
            if extra.iter().any(|p| required_keyword(p) && &p.name == name) {
                inner.local_bindings.insert(name.clone(), ty.clone());
            }
        }
    }

    /// `Fn` parameters for a stored lambda. A hole is untyped; parameters
    /// past the longest call stay off the type.
    pub(super) fn fn_params(&self, params: &[Symbol]) -> Vec<crate::ty::Param> {
        let mut out = Vec::new();
        for (name, slot) in params.iter().zip(self.positional.iter()) {
            let ty = slot.clone().unwrap_or(Ty::Untyped);
            out.push(crate::ty::Param {
                name: name.clone(),
                ty: std::sync::Arc::new(ty),
                kind: crate::ty::ParamKind::Required,
            });
        }
        for (name, ty) in &self.keywords {
            out.push(crate::ty::Param {
                name: name.clone(),
                ty: std::sync::Arc::new(ty.clone()),
                kind: crate::ty::ParamKind::Keyword { required: true },
            });
        }
        out
    }
}

/// Fold call-site evidence into `stored`. True when a parameter type moved.
pub(super) fn merge(stored: &mut Stored, evidence: &[Evidence]) -> bool {
    let mut changed = false;
    for ev in evidence {
        if let Some(pos) = &ev.positional {
            changed |= merge_positional(&mut stored.positional, pos);
        }
        for (name, ty) in &ev.keywords {
            if !stored.keyword_names.iter().any(|n| n == name) {
                continue;
            }
            changed |= merge_keyword(&mut stored.keywords, name, ty);
        }
    }
    changed
}

fn merge_positional(acc: &mut Vec<Option<Ty>>, next: &[Option<Ty>]) -> bool {
    let mut changed = false;
    for (i, slot) in next.iter().enumerate() {
        let Some(ty) = slot else { continue };
        if matches!(ty, Ty::Var { .. } | Ty::Untyped) {
            continue;
        }
        if i >= acc.len() {
            acc.resize(i, None);
            acc.push(Some(ty.clone()));
            changed = true;
            continue;
        }
        match &acc[i] {
            None => {
                acc[i] = Some(ty.clone());
                changed = true;
            }
            Some(prev) => {
                let joined = union_of(prev.clone(), ty.clone());
                if &joined != prev {
                    acc[i] = Some(joined);
                    changed = true;
                }
            }
        }
    }
    changed
}

fn merge_keyword(acc: &mut Vec<(Symbol, Ty)>, name: &Symbol, ty: &Ty) -> bool {
    if matches!(ty, Ty::Var { .. } | Ty::Untyped) {
        return false;
    }
    if let Some((_, prev)) = acc.iter_mut().find(|(n, _)| n == name) {
        let joined = union_of(prev.clone(), ty.clone());
        if &joined != prev {
            *prev = joined;
            return true;
        }
        return false;
    }
    acc.push((name.clone(), ty.clone()));
    true
}

pub(super) fn seeds_of(stored: &Stored) -> Seeds {
    Seeds { positional: stored.positional.clone(), keywords: stored.keywords.clone() }
}

pub(super) fn calls_in(expr: &Expr, name: &Symbol) -> Vec<Evidence> {
    let mut out = Vec::new();
    let mut stop = false;
    walk(expr, name, &mut out, &mut stop);
    out
}

fn walk(expr: &Expr, name: &Symbol, out: &mut Vec<Evidence>, stop: &mut bool) {
    if *stop {
        return;
    }
    if let Some(before) = value_before_local_write(expr, name) {
        walk(before, name, out, stop);
        *stop = true;
        return;
    }
    match &*expr.node {
        ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } => {
            let shadows = params.iter().chain(rest_param.iter()).chain(block_param.iter()).any(|p| p == name)
                || extra_params.iter().any(|p| &p.name == name);
            if !shadows {
                walk(body, name, out, stop);
            }
        }
        ExprNode::Send { recv, method, args, block, .. }
            if block.is_none()
                && matches!(method.as_str(), "call" | "[]")
                && recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Var { name: n, .. } if n == name)) =>
        {
            if let Some(ev) = evidence(args) {
                out.push(ev);
            }
            if let Some(r) = recv {
                walk(r, name, out, stop);
            }
            for arg in args {
                walk(arg, name, out, stop);
            }
        }
        _ => expr.node.for_each_child(&mut |child| walk(child, name, out, stop)),
    }
}

fn evidence(args: &[Expr]) -> Option<Evidence> {
    let mut positional = Vec::new();
    let mut keywords = Vec::new();
    let mut positions_known = true;
    for arg in args {
        match &*arg.node {
            ExprNode::Splat { .. } => {
                positions_known = false;
                break;
            }
            ExprNode::KeywordSplat { .. } | ExprNode::ForwardKeywords | ExprNode::ForwardKeywordsWithPairs { .. } => {
                break;
            }
            ExprNode::Hash { entries, kwargs: true } => {
                for (key, value) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: name } } = &*key.node else {
                        continue;
                    };
                    if let Some(ty) = typed(&value.ty) {
                        keywords.push((name.clone(), ty));
                    }
                }
                break;
            }
            _ => positional.push(typed(&arg.ty)),
        }
    }
    let positional = positions_known.then_some(positional);
    let any = positional.as_ref().is_some_and(|p| p.iter().any(|t| t.is_some())) || !keywords.is_empty();
    any.then_some(Evidence { positional, keywords })
}

fn typed(ty: &Option<Ty>) -> Option<Ty> {
    match ty {
        Some(Ty::Var { .. } | Ty::Untyped) | None => None,
        Some(ty) => Some(ty.clone()),
    }
}

/// Names this statement assigns, so the caller can drop a stored lambda
/// that this statement replaced. Only the statement's own binding, not
/// assignments nested where the previous value is still what `.call` sees.
pub(super) fn direct_var_assign(expr: &Expr) -> Option<(&Symbol, &Expr)> {
    let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*expr.node else {
        return None;
    };
    Some((name, value))
}

pub(super) fn remember(
    lambdas: &mut HashMap<Symbol, Stored>,
    index: usize,
    expr: &Expr,
    ctx: &Ctx,
) {
    let Some((name, value)) = direct_var_assign(expr) else { return };
    if let Some(stored) = stored_from(index, value, ctx) {
        lambdas.insert(name.clone(), stored);
    } else {
        lambdas.remove(name);
    }
}

/// A write that runs in this statement replaces the stored lambda, so a
/// later `.call` must not retype that lambda and put it back into the
/// local. Returns the names removed from the map. The caller drops those
/// bindings only when this statement is not a join: an `if` that writes
/// the name on one side has to keep the lambda for the other side, or
/// that side becomes nil. A lambda that is only created stays a barrier.
/// A block on a call runs with the call, and so does the body of a stored
/// lambda this statement invokes. The statement's own direct `name = …`
/// is `remember`'s job.
pub(super) fn forget_nested_writes(
    lambdas: &mut HashMap<Symbol, Stored>,
    exprs: &[Expr],
    index: usize,
) -> Vec<Symbol> {
    if lambdas.is_empty() {
        return Vec::new();
    }
    let expr = &exprs[index];
    let mut names = Vec::new();
    let none = std::collections::HashSet::new();
    collect_executed_assigns(expr, true, &none, &mut names);
    let mut seen = std::collections::HashSet::new();
    let mut queue: Vec<Symbol> = lambdas
        .keys()
        .filter(|name| !calls_in(expr, name).is_empty())
        .cloned()
        .collect();
    while let Some(name) = queue.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(value) = stored_lambda_expr(exprs, lambdas, &name) else {
            continue;
        };
        collect_executed_lambda(value, &none, &mut names);
        for other in lambdas.keys() {
            if !seen.contains(other) && !calls_in(value, other).is_empty() {
                queue.push(other.clone());
            }
        }
    }
    names.retain(|name| lambdas.remove(name).is_some());
    names
}

fn stored_lambda_expr<'a>(
    exprs: &'a [Expr],
    lambdas: &HashMap<Symbol, Stored>,
    name: &Symbol,
) -> Option<&'a Expr> {
    let stored = lambdas.get(name)?;
    let (bound, value) = direct_var_assign(exprs.get(stored.index)?)?;
    (bound == name).then_some(value)
}

fn collect_executed_assigns(
    expr: &Expr,
    root: bool,
    shadowed: &std::collections::HashSet<Symbol>,
    out: &mut Vec<Symbol>,
) {
    match &*expr.node {
        ExprNode::Lambda { .. } => return,
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            if !root && !shadowed.contains(name) {
                out.push(name.clone());
            }
            collect_executed_assigns(value, false, shadowed, out);
            return;
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(recv) = recv {
                collect_executed_assigns(recv, false, shadowed, out);
            }
            for arg in args {
                collect_executed_assigns(arg, false, shadowed, out);
            }
            if let Some(block) = block {
                collect_executed_lambda(block, shadowed, out);
            }
            return;
        }
        _ => {}
    }
    push_other_local_writes(expr, shadowed, out);
    expr.node.for_each_child(&mut |child| collect_executed_assigns(child, false, shadowed, out));
}

/// An attached block or an invoked lambda runs now. Its parameters
/// shadow the outer local of the same name.
fn collect_executed_lambda(
    expr: &Expr,
    outer: &std::collections::HashSet<Symbol>,
    out: &mut Vec<Symbol>,
) {
    let ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } = &*expr.node else {
        collect_executed_assigns(expr, false, outer, out);
        return;
    };
    let mut shadowed = outer.clone();
    shadowed.extend(params.iter().cloned());
    shadowed.extend(rest_param.iter().cloned());
    shadowed.extend(block_param.iter().cloned());
    shadowed.extend(extra_params.iter().map(|p| p.name.clone()));
    collect_executed_assigns(body, false, &shadowed, out);
}

/// The subexpression evaluated before `name` is written, when this node
/// writes it. Calls there still see the stored lambda. Calls after the
/// write do not.
fn value_before_local_write<'a>(expr: &'a Expr, name: &Symbol) -> Option<&'a Expr> {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Var { name: bound, .. }, value }
        | ExprNode::OpAssign { target: LValue::Var { name: bound, .. }, value, .. }
        | ExprNode::Let { name: bound, value, .. }
            if bound == name =>
        {
            Some(value)
        }
        ExprNode::MultiAssign { targets, value }
            if targets.iter().any(|t| matches!(t, LValue::Var { name: n, .. } if n == name)) =>
        {
            Some(value)
        }
        ExprNode::MatchRequired { value, pattern, .. } | ExprNode::MatchPredicate { value, pattern, .. } => {
            binds_name(pattern, name).then_some(value)
        }
        ExprNode::CaseMatch { scrutinee, arms, .. } => {
            arms.iter().any(|arm| binds_name(&arm.pattern, name)).then_some(scrutinee)
        }
        ExprNode::BeginRescue { body, rescues, .. }
            if rescues.iter().any(|r| r.binding.as_ref() == Some(name)) =>
        {
            Some(body)
        }
        _ => None,
    }
}

fn binds_name(pattern: &crate::expr::MatchPattern, name: &Symbol) -> bool {
    let mut names = Vec::new();
    pattern.bound_names(&mut names);
    names.iter().any(|n| n == name)
}

fn push_other_local_writes(
    expr: &Expr,
    shadowed: &std::collections::HashSet<Symbol>,
    out: &mut Vec<Symbol>,
) {
    match &*expr.node {
        ExprNode::OpAssign { target: LValue::Var { name, .. }, .. } | ExprNode::Let { name, .. } => {
            if !shadowed.contains(name) {
                out.push(name.clone());
            }
        }
        ExprNode::MultiAssign { targets, .. } => {
            for target in targets {
                if let LValue::Var { name, .. } = target {
                    if !shadowed.contains(name) {
                        out.push(name.clone());
                    }
                }
            }
        }
        ExprNode::CaseMatch { arms, .. } => {
            for arm in arms {
                arm.pattern.bound_names(out);
            }
        }
        ExprNode::MatchRequired { pattern, .. } | ExprNode::MatchPredicate { pattern, .. } => {
            pattern.bound_names(out);
        }
        ExprNode::BeginRescue { rescues, .. } => {
            for rescue in rescues {
                out.extend(rescue.binding.iter().cloned());
            }
        }
        _ => {}
    }
}
