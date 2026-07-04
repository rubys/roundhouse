//! Scope-call normalization — the lowering that lets ActiveRecord scope
//! chains run against the metaprogramming-free `ActiveRecord::Relation`
//! runtime (see project_lobsters_benchmark_parity_plan).
//!
//! A model `scope :name, ->(args){ body }` lowers (in `push_scope_methods`)
//! to a class method `def self.name(args, _rel = ActiveRecord::Relation.new(self))`.
//! For `Story.base(u).positive_ranked` to work without `method_missing`,
//! every scope INVOCATION is rewritten so the scope is an ordinary class
//! method taking the current relation as a trailing argument:
//!
//!   recv.scope(args)        (recv is a relation)  ->  Model.scope(args, recv)
//!   Model.scope(args)       (call on the class)   ->  Model.scope(args)   [default rel]
//!   <implicit>.scope(args)  (inside a scope body) ->  Model.scope(args, _rel)
//!
//! Relation built-ins (`where`/`order`/`limit`/…) stay `recv.method(args)`;
//! a `Model.where(...)` / `Model.all` chain-start (no scope) is seeded with
//! `ActiveRecord::Relation.new(Model)` so it, too, is chainable.

use std::collections::{HashMap, HashSet};

use crate::dialect::{Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};

/// model class id -> (scope name -> the scope's user params, in order).
/// The params are the lambda's own parameters (NOT the synthesized trailing
/// `__rel`); the rewriter reads them to pad omitted leading args so a
/// threaded relation lands in the `__rel` slot (see `thread_rel`).
pub type ScopeRegistry = HashMap<ClassId, HashMap<Symbol, Vec<Param>>>;

/// Build `model -> {scope name -> params}` from the app's models.
pub fn build_scope_registry(models: &[Model]) -> ScopeRegistry {
    let mut reg: ScopeRegistry = HashMap::new();
    for m in models {
        let map = reg.entry(m.name.clone()).or_default();
        for item in &m.body {
            if let ModelBodyItem::Scope { scope, .. } = item {
                map.insert(scope.name.clone(), scope.params.clone());
            }
        }
    }
    reg
}

/// The set of model class ids (so a `Const([M])` receiver can be recognized
/// as a class-level scope call vs. an arbitrary constant).
pub fn model_set(models: &[Model]) -> HashSet<ClassId> {
    models.iter().map(|m| m.name.clone()).collect()
}

/// True when any model declares a scope (the whole pass is a no-op
/// otherwise — e.g. the scope-free blog).
pub fn any_scopes(scopes: &ScopeRegistry) -> bool {
    scopes.values().any(|s| !s.is_empty())
}

/// Union of every scope name across all models — a cheap pre-filter so a
/// body that names no scope is left completely untouched.
pub fn all_scope_names(scopes: &ScopeRegistry) -> HashSet<Symbol> {
    scopes.values().flat_map(|m| m.keys().cloned()).collect()
}

/// True if `expr` (or a descendant) calls a method whose name is a scope.
pub fn mentions_scope(expr: &Expr, names: &HashSet<Symbol>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, names: &HashSet<Symbol>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { method, .. } = &*e.node {
            if names.contains(method) {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, names, found));
    }
    walk(expr, names, &mut found);
    found
}

/// Relation methods that return a Relation — calls on them stay on the
/// receiver and the chain keeps its model. Terminals / Enumerable methods
/// (`to_a`/`first`/`map`/`pluck`/…) are deliberately absent: they end the
/// relation, so model-tracking stops after them (a scope can't follow).
fn is_relation_chain_method(name: &str) -> bool {
    matches!(
        name,
        "where"
            | "not"
            | "order"
            | "limit"
            | "offset"
            | "group"
            | "having"
            | "joins"
            | "left_outer_joins"
            | "select"
            | "distinct"
            | "includes"
            | "preload"
            | "eager_load"
            | "merge"
            | "none"
    )
}

/// Shared lookup tables; `scope_body` is `Some((self_model, rel_param))`
/// when rewriting a scope's own body (so implicit-self query roots thread
/// the relation parameter), `None` at every other call site.
pub struct Ctx<'a> {
    pub scopes: &'a ScopeRegistry,
    pub models: &'a HashSet<ClassId>,
    pub scope_body: Option<(ClassId, Symbol)>,
}

impl Ctx<'_> {
    fn scope_of(&self, model: &ClassId, method: &Symbol) -> bool {
        self.scopes.get(model).is_some_and(|s| s.contains_key(method))
    }
    /// The scope's own (user) params, so the rewriter can pad omitted
    /// leading args when threading the relation.
    fn scope_params(&self, model: &ClassId, method: &Symbol) -> Option<&Vec<Param>> {
        self.scopes.get(model).and_then(|s| s.get(method))
    }
    /// A copy of self with no scope-body relation (for args / blocks /
    /// non-receiver subtrees, which root at their own constants).
    fn at_callsite(&self) -> Ctx<'_> {
        Ctx { scopes: self.scopes, models: self.models, scope_body: None }
    }
}

fn syn(span: crate::span::Span, node: ExprNode) -> Expr {
    Expr::new(span, node)
}

/// Append the threaded relation to a scope call's args so it lands in the
/// synthesized trailing `__rel` slot. A scope with leading OPTIONAL params
/// (`hottest(user = nil, exclude_tags = nil)`) called with fewer args than
/// it declares — e.g. bare `hottest` in `front_page` — would otherwise bind
/// the relation to the FIRST param. Pad the skipped leading params with
/// their own defaults (Ruby's behavior for the omitted call) before pushing
/// the relation, so `hottest` → `Story.hottest(nil, nil, __rel)` not
/// `Story.hottest(__rel)`. `leading` is the scope's user params (no `__rel`).
fn thread_rel(mut args: Vec<Expr>, rel: Expr, leading: Option<&Vec<Param>>, span: crate::span::Span) -> Vec<Expr> {
    if let Some(params) = leading {
        for p in params.iter().skip(args.len()) {
            let filler = p
                .default
                .clone()
                .unwrap_or_else(|| syn(span, ExprNode::Lit { value: Literal::Nil }));
            args.push(filler);
        }
    }
    args.push(rel);
    args
}

fn const_expr(span: crate::span::Span, model: &ClassId) -> Expr {
    let path: Vec<Symbol> = model.0.as_str().split("::").map(Symbol::from).collect();
    syn(span, ExprNode::Const { path })
}

fn var_expr(span: crate::span::Span, name: &Symbol) -> Expr {
    syn(span, ExprNode::Var { id: VarId(0), name: name.clone() })
}

/// `ActiveRecord::Relation.new(Model)`.
fn relation_new(span: crate::span::Span, model: &ClassId) -> Expr {
    let recv = syn(
        span,
        ExprNode::Const { path: vec![Symbol::from("ActiveRecord"), Symbol::from("Relation")] },
    );
    syn(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("new"),
            args: vec![const_expr(span, model)],
            block: None,
            parenthesized: true,
        },
    )
}

/// If `expr` is a bare `Const([M])` for a known model, return that model.
fn const_model(expr: &Expr, models: &HashSet<ClassId>) -> Option<ClassId> {
    if let ExprNode::Const { path } = &*expr.node {
        let joined = ClassId(Symbol::from(
            path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        ));
        if models.contains(&joined) {
            return Some(joined);
        }
    }
    None
}

/// Local variable -> relation model, accumulated as a method body's
/// statements are processed in order (so `q = Story.base(u); q.not_deleted`
/// resolves `not_deleted` against `q`'s Story relation).
type Locals = HashMap<Symbol, ClassId>;

/// Rewrite scope chains in `expr` (in place). Returns the relation-model of
/// the whole expression when it evaluates to a Relation of a known model.
pub fn rewrite(expr: &mut Expr, ctx: &Ctx, locals: &mut Locals) -> Option<ClassId> {
    match &*expr.node {
        // Statement sequence: thread `locals` left-to-right; the Seq's value
        // (and model) is its last statement.
        ExprNode::Seq { .. } => {
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Seq { exprs } = node else { unreachable!() };
            let mut last = None;
            let mut out = Vec::with_capacity(exprs.len());
            for mut e in exprs {
                last = rewrite(&mut e, ctx, locals);
                out.push(e);
            }
            *expr.node = ExprNode::Seq { exprs: out };
            last
        }
        // `name = value`: record the local's relation model (if any).
        ExprNode::Assign { .. } => {
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Assign { target, mut value } = node else { unreachable!() };
            let m = rewrite(&mut value, ctx, locals);
            if let crate::expr::LValue::Var { name, .. } = &target {
                match &m {
                    Some(model) => {
                        locals.insert(name.clone(), model.clone());
                    }
                    None => {
                        locals.remove(name);
                    }
                }
            }
            *expr.node = ExprNode::Assign { target, value };
            m
        }
        ExprNode::Send { .. } => rewrite_send(expr, ctx, locals),
        _ => {
            // Any other node (If/BoolOp/Case/…): recurse children, keeping
            // the same ctx + locals so the relation thread survives across
            // branches (a scope body's `if … q.preload … else q.not_deleted`).
            expr.node.for_each_child_mut(&mut |c| {
                rewrite(c, ctx, locals);
            });
            None
        }
    }
}

fn rewrite_send(expr: &mut Expr, ctx: &Ctx, locals: &mut Locals) -> Option<ClassId> {
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { recv, method, mut args, mut block, parenthesized } = node else {
        unreachable!()
    };

    // Args + block are independent subtrees: they root at their own
    // constants (drop the scope-body relation), but may still read outer
    // locals.
    let arg_ctx = ctx.at_callsite();
    for a in &mut args {
        rewrite(a, &arg_ctx, locals);
    }
    if let Some(b) = &mut block {
        rewrite(b, &arg_ctx, locals);
    }

    let put = |span: crate::span::Span, recv, method, args, block, parenthesized| -> Expr {
        syn(span, ExprNode::Send { recv, method, args, block, parenthesized })
    };

    match recv {
        None => {
            if let Some((self_model, rel)) = &ctx.scope_body {
                // Bare `all` inside a scope body IS the current relation —
                // not `Model.all` (which would hit Base.all and return an
                // Array, breaking the chain). Replace with the rel param.
                if method.as_str() == "all" && args.is_empty() && block.is_none() {
                    *expr = var_expr(span, rel);
                    return Some(self_model.clone());
                }
                if ctx.scope_of(self_model, &method) {
                    let leading = ctx.scope_params(self_model, &method);
                    let new_args = thread_rel(args, var_expr(span, rel), leading, span);
                    *expr = put(span, Some(const_expr(span, self_model)), method, new_args, block, true);
                    return Some(self_model.clone());
                }
                if is_relation_chain_method(method.as_str()) {
                    *expr = put(span, Some(var_expr(span, rel)), method, args, block, parenthesized);
                    return Some(self_model.clone());
                }
            }
            *expr = put(span, None, method, args, block, parenthesized);
            None
        }
        Some(mut r) => {
            // Class-level call on a model constant.
            if let Some(m) = const_model(&r, ctx.models) {
                if ctx.scope_of(&m, &method) {
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return Some(m);
                }
                if is_relation_chain_method(method.as_str()) || method.as_str() == "all" {
                    let seed = relation_new(span, &m);
                    if method.as_str() == "all" {
                        *expr = seed;
                    } else {
                        *expr = put(span, Some(seed), method, args, block, parenthesized);
                    }
                    return Some(m);
                }
                *expr = put(span, Some(r), method, args, block, parenthesized);
                return None;
            }

            // Receiver model: a local var holding a relation, else the
            // (rewritten) receiver chain's model.
            let r_model = match &*r.node {
                ExprNode::Var { name, .. } => locals.get(name).cloned(),
                _ => rewrite(&mut r, ctx, locals),
            };

            if let Some(mr) = r_model {
                if ctx.scope_of(&mr, &method) {
                    let leading = ctx.scope_params(&mr, &method);
                    let new_args = thread_rel(args, r, leading, span);
                    *expr = put(span, Some(const_expr(span, &mr)), method, new_args, block, true);
                    return Some(mr);
                }
                if is_relation_chain_method(method.as_str()) {
                    *expr = put(span, Some(r), method, args, block, parenthesized);
                    return Some(mr);
                }
                *expr = put(span, Some(r), method, args, block, parenthesized);
                return None;
            }

            *expr = put(span, Some(r), method, args, block, parenthesized);
            None
        }
    }
}

/// Rewrite a scope body: implicit-self query roots thread `rel_param`.
pub fn rewrite_scope_body(
    body: &mut Expr,
    self_model: &ClassId,
    rel_param: &Symbol,
    scopes: &ScopeRegistry,
    models: &HashSet<ClassId>,
) {
    let ctx = Ctx {
        scopes,
        models,
        scope_body: Some((self_model.clone(), rel_param.clone())),
    };
    let mut locals = Locals::new();
    rewrite(body, &ctx, &mut locals);
}

/// Rewrite a non-scope-body expression (controller action, library-class
/// method, model instance method): scope chains root at a model constant.
pub fn rewrite_call_site(expr: &mut Expr, scopes: &ScopeRegistry, models: &HashSet<ClassId>) {
    let ctx = Ctx { scopes, models, scope_body: None };
    let mut locals = Locals::new();
    rewrite(expr, &ctx, &mut locals);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn span() -> Span {
        Span::synthetic()
    }
    fn int_lit(n: i64) -> Expr {
        Expr::new(span(), ExprNode::Lit { value: Literal::Int { value: n } })
    }
    fn rel_marker() -> Expr {
        Expr::new(span(), ExprNode::Var { id: VarId(0), name: Symbol::from("__rel") })
    }
    fn is_nil(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
    }
    fn is_rel(e: &Expr) -> bool {
        matches!(&*e.node, ExprNode::Var { name, .. } if name.as_str() == "__rel")
    }

    #[test]
    fn thread_rel_pads_omitted_optional_leading_params() {
        // `hottest(user = nil, exclude_tags = nil)` called bare → the rel
        // must land in the 3rd (__rel) slot, with the two optionals padded.
        let leading = vec![
            Param::with_default(Symbol::from("user"), Expr::new(span(), ExprNode::Lit { value: Literal::Nil })),
            Param::with_default(Symbol::from("exclude_tags"), Expr::new(span(), ExprNode::Lit { value: Literal::Nil })),
        ];
        let out = thread_rel(vec![], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 3);
        assert!(is_nil(&out[0]) && is_nil(&out[1]) && is_rel(&out[2]));
    }

    #[test]
    fn thread_rel_pads_with_the_params_own_default() {
        // `low_scoring(max = 5)` bare → pad max with its default `5`, not nil.
        let leading = vec![Param::with_default(Symbol::from("max"), int_lit(5))];
        let out = thread_rel(vec![], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2);
        assert!(matches!(&*out[0].node, ExprNode::Lit { value: Literal::Int { value: 5 } }));
        assert!(is_rel(&out[1]));
    }

    #[test]
    fn thread_rel_no_padding_when_all_supplied() {
        // `base(user)` — one required param supplied → just append the rel.
        let leading = vec![Param::positional(Symbol::from("user"))];
        let supplied = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("user") });
        let out = thread_rel(vec![supplied], rel_marker(), Some(&leading), span());
        assert_eq!(out.len(), 2);
        assert!(is_rel(&out[1]));
    }

    #[test]
    fn thread_rel_scopeless_just_appends() {
        // A scope with no user params (`positive_ranked`) → append only.
        let out = thread_rel(vec![], rel_marker(), Some(&vec![]), span());
        assert_eq!(out.len(), 1);
        assert!(is_rel(&out[0]));
    }
}
