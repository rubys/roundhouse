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

use crate::dialect::{Association, Model, ModelBodyItem, Param};
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::pluralize_snake;

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

/// Per-model association facts the chain rewriter consumes once a chain's
/// model is known: `joins(:assoc)` expands to its JOIN SQL, and a
/// `belongs_to`-named hash key in `where`/`not` rewrites to the foreign-key
/// column (the runtime Relation sees only columns and SQL — the compiler is
/// where association knowledge lives).
#[derive(Default)]
pub struct AssocRegistry {
    /// (model, association name) -> `"<target_table> ON <cond>"`; the
    /// rewrite prefixes `INNER JOIN` / `LEFT OUTER JOIN` by call. Direct
    /// `belongs_to`/`has_many`/`has_one`, plus resolvable `has_many
    /// :through` — a through tail carries its own second `INNER JOIN`, so
    /// a `left_outer_joins(:through_assoc)` would outer-join only the
    /// first hop (no such call exists in the exercised apps; revisit if
    /// one appears). Habtm and unresolvable through shapes stay absent,
    /// so their `joins(:sym)` is left untouched (visible at runtime
    /// rather than silently mis-joined).
    join_tails: HashMap<(ClassId, Symbol), String>,
    /// (model, belongs_to name) -> foreign-key column, for
    /// `where(user: user)` -> `where(user_id: user && user.id)`.
    belongs_to_fk: HashMap<(ClassId, Symbol), Symbol>,
}

impl AssocRegistry {
    fn join_tail(&self, model: &ClassId, assoc: &Symbol) -> Option<&String> {
        self.join_tails.get(&(model.clone(), assoc.clone()))
    }
    fn belongs_to_fk(&self, model: &ClassId, assoc: &Symbol) -> Option<&Symbol> {
        self.belongs_to_fk.get(&(model.clone(), assoc.clone()))
    }
}

/// Build the association registry. Table names use the same
/// `pluralize_snake` the synthesized `table_name` methods use, so the
/// generated SQL and the runtime agree by construction.
pub fn build_assoc_registry(models: &[Model]) -> AssocRegistry {
    let mut reg = AssocRegistry::default();
    for m in models {
        let own = pluralize_snake(m.name.0.as_str());
        for a in m.associations() {
            match a {
                Association::BelongsTo { name, target, foreign_key, .. } => {
                    let t = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.id = {own}.{foreign_key}"),
                    );
                    reg.belongs_to_fk
                        .insert((m.name.clone(), name.clone()), foreign_key.clone());
                }
                Association::HasMany { name, target, foreign_key, through: None, .. }
                | Association::HasOne { name, target, foreign_key, .. } => {
                    let t = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!("{t} ON {t}.{foreign_key} = {own}.id"),
                    );
                }
                // `has_many :through`: two hops, owner-side direction
                // (`Tag.joins(:stories)` → JOIN taggings ON tag_id, JOIN
                // stories ON story_id). Same fk resolution as the
                // through-reader lowering: the through association names
                // the join model; its `belongs_to` matching the assoc's
                // target class supplies the source fk (survives `source:`
                // renames, which ingest folds into `target`).
                Association::HasMany { name, target, through: Some(thr_name), .. } => {
                    let Some(Association::HasMany { target: thr_target, foreign_key: thr_fk, .. }) =
                        m.associations().find(|a| {
                            matches!(a, Association::HasMany { name, .. } if name == thr_name)
                        })
                    else {
                        continue;
                    };
                    let Some(thr_model) = models.iter().find(|tm| &tm.name == thr_target) else {
                        continue;
                    };
                    let Some(Association::BelongsTo { foreign_key: src_fk, .. }) =
                        thr_model.associations().find(|a| {
                            matches!(a, Association::BelongsTo { target: t, .. } if t == target)
                        })
                    else {
                        continue;
                    };
                    let thr_table = pluralize_snake(thr_target.0.as_str());
                    let target_table = pluralize_snake(target.0.as_str());
                    reg.join_tails.insert(
                        (m.name.clone(), name.clone()),
                        format!(
                            "{thr_table} ON {thr_table}.{thr_fk} = {own}.id \
                             INNER JOIN {target_table} ON {target_table}.id = {thr_table}.{src_fk}"
                        ),
                    );
                }
                _ => {}
            }
        }
    }
    reg
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

/// True if `expr` (or a descendant) starts a query chain directly on a
/// known model constant (`Vote.where(...)`, `Story.all`). Scope-free
/// bodies can still hold such chains — the arel inline pass refuses a
/// where-hash whose value isn't statically scalar (an Array means `IN`,
/// nil means `IS NULL`, only runtime knows), so those chains reach emit
/// as plain sends and must be seeded with a Relation to run.
pub fn mentions_model_chain_start(expr: &Expr, models: &HashSet<ClassId>) -> bool {
    let mut found = false;
    fn walk(e: &Expr, models: &HashSet<ClassId>, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node {
            if (is_relation_chain_method(method.as_str()) || method.as_str() == "all")
                && const_model(r, models).is_some()
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, models, found));
    }
    walk(expr, models, &mut found);
    found
}

/// True if `expr` (or a descendant) calls a relation chain method with no
/// receiver (or on explicit `self` — same thing spelled out) — the
/// implicit-self query root a model's own class method uses
/// (`self.where(key: key)` in `Keystore.value_for`). Only meaningful for
/// bodies rewritten with `class_self` set.
pub fn mentions_bare_chain_start(expr: &Expr) -> bool {
    let mut found = false;
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv, method, .. } = &*e.node {
            let self_rooted = match recv {
                None => true,
                Some(r) => matches!(&*r.node, ExprNode::SelfRef),
            };
            if self_rooted
                && (is_relation_chain_method(method.as_str()) || method.as_str() == "all")
            {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(expr, &mut found);
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
    pub assocs: &'a AssocRegistry,
    pub scope_body: Option<(ClassId, Symbol)>,
    /// `Some(model)` when rewriting a model's own CLASS method (a
    /// user-written `def self.x`): a bare `where(...)`/`all` there is an
    /// implicit-self query root (`Keystore.value_for`'s `where(key:
    /// key).limit(1)`), so it seeds `Relation.new(Model)` — like a scope
    /// body, but with no `__rel` parameter to thread.
    pub class_self: Option<ClassId>,
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
        Ctx {
            scopes: self.scopes,
            models: self.models,
            assocs: self.assocs,
            scope_body: None,
            class_self: self.class_self.clone(),
        }
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

/// In-place argument rewrites for a relation chain method once the chain's
/// model is known:
///
///   joins(:hidings)      -> joins("INNER JOIN hidden_stories ON …")
///   where(user: user)    -> where(user_id: user && user.id)
///   not(user: user)      -> likewise (the `where.not` lowering)
///
/// Unknown association names (and `:through`) are left untouched. A hash
/// key renames whenever it names a `belongs_to`; its VALUE is narrowed to
/// `v && v.id` only for plain reads (Var/Ivar — evaluating twice is free);
/// literals ride as-is, so `where(user: nil)` stays `user_id IS NULL`, and
/// call-expression values are left alone rather than double-evaluated.
fn lower_relation_args(model: &ClassId, method: &Symbol, args: &mut [Expr], ctx: &Ctx) {
    match method.as_str() {
        "joins" | "left_outer_joins" => {
            let kind = if method.as_str() == "joins" { "INNER JOIN" } else { "LEFT OUTER JOIN" };
            for a in args {
                let ExprNode::Lit { value: Literal::Sym { value } } = &*a.node else { continue };
                if let Some(tail) = ctx.assocs.join_tail(model, value) {
                    *a.node = ExprNode::Lit { value: Literal::Str { value: format!("{kind} {tail}") } };
                }
            }
        }
        "where" | "not" | "find_by" => {
            for a in args.iter_mut() {
                let span = a.span;
                let ExprNode::Hash { entries, .. } = &mut *a.node else { continue };
                for (k, v) in entries.iter_mut() {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                        continue;
                    };
                    let Some(fk) = ctx.assocs.belongs_to_fk(model, key) else { continue };
                    *k.node = ExprNode::Lit { value: Literal::Sym { value: fk.clone() } };
                    if matches!(&*v.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
                        let val = std::mem::replace(
                            v,
                            syn(span, ExprNode::Lit { value: Literal::Nil }),
                        );
                        let id_read = syn(
                            span,
                            ExprNode::Send {
                                recv: Some(val.clone()),
                                method: Symbol::from("id"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        );
                        *v = syn(
                            span,
                            ExprNode::BoolOp {
                                op: BoolOpKind::And,
                                surface: BoolOpSurface::Symbol,
                                left: val,
                                right: id_read,
                            },
                        );
                    }
                }
            }
        }
        _ => {}
    }
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

    // `self.where(...)` in a scope body / model class method is the same
    // implicit-self query root as bare `where(...)` — Ruby just makes the
    // receiver visible. Normalize to the receiver-less form so the None
    // arm's rooting logic serves both spellings (`Keystore.value_for`'s
    // `self.where(key: key)` seeds exactly like `where(key: key)`).
    let recv = match recv {
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {
            let self_model =
                ctx.scope_body.as_ref().map(|(m, _)| m).or(ctx.class_self.as_ref());
            match self_model {
                Some(m)
                    if is_relation_chain_method(method.as_str())
                        || method.as_str() == "all"
                        || ctx.scope_of(m, &method) =>
                {
                    None
                }
                _ => Some(r),
            }
        }
        other => other,
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
                    lower_relation_args(self_model, &method, &mut args, ctx);
                    *expr = put(span, Some(var_expr(span, rel)), method, args, block, parenthesized);
                    return Some(self_model.clone());
                }
            }
            if let Some(self_model) = ctx.class_self.clone() {
                // Implicit-self query root in a model's own class method:
                // `all` IS a fresh relation; a bare scope call is the
                // class-level form; a bare chain method seeds a new
                // relation (there's no `__rel` param here to thread).
                if method.as_str() == "all" && args.is_empty() && block.is_none() {
                    *expr = relation_new(span, &self_model);
                    return Some(self_model);
                }
                if ctx.scope_of(&self_model, &method) {
                    *expr =
                        put(span, Some(const_expr(span, &self_model)), method, args, block, true);
                    return Some(self_model);
                }
                if is_relation_chain_method(method.as_str()) {
                    lower_relation_args(&self_model, &method, &mut args, ctx);
                    let seed = relation_new(span, &self_model);
                    *expr = put(span, Some(seed), method, args, block, parenthesized);
                    return Some(self_model);
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
                        lower_relation_args(&m, &method, &mut args, ctx);
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
                    lower_relation_args(&mr, &method, &mut args, ctx);
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
    assocs: &AssocRegistry,
) {
    let ctx = Ctx {
        scopes,
        models,
        assocs,
        scope_body: Some((self_model.clone(), rel_param.clone())),
        class_self: None,
    };
    let mut locals = Locals::new();
    rewrite(body, &ctx, &mut locals);
}

/// Rewrite a non-scope-body expression (controller action, library-class
/// method, model instance method): scope chains root at a model constant.
/// `class_self` carries the model when the body is that model's own
/// class method, so bare implicit-self roots (`where(key: key)`) seed.
pub fn rewrite_call_site(
    expr: &mut Expr,
    scopes: &ScopeRegistry,
    models: &HashSet<ClassId>,
    assocs: &AssocRegistry,
    class_self: Option<&ClassId>,
) {
    let ctx = Ctx { scopes, models, assocs, scope_body: None, class_self: class_self.cloned() };
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

    // ---- lower_relation_args ----------------------------------------

    fn story() -> ClassId {
        ClassId(Symbol::from("Story"))
    }

    /// Story: has_many :hidings (HiddenStory, fk story_id);
    /// HiddenStory: belongs_to :user (fk user_id).
    fn assoc_fixture() -> AssocRegistry {
        let mut reg = AssocRegistry::default();
        reg.join_tails.insert(
            (story(), Symbol::from("hidings")),
            "hidden_stories ON hidden_stories.story_id = stories.id".to_string(),
        );
        reg.belongs_to_fk.insert(
            (ClassId(Symbol::from("HiddenStory")), Symbol::from("user")),
            Symbol::from("user_id"),
        );
        reg
    }

    fn ctx_with<'a>(
        scopes: &'a ScopeRegistry,
        models: &'a HashSet<ClassId>,
        assocs: &'a AssocRegistry,
    ) -> Ctx<'a> {
        Ctx { scopes, models, assocs, scope_body: None, class_self: None }
    }

    fn sym_lit(s: &str) -> Expr {
        Expr::new(span(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
    }

    #[test]
    fn joins_sym_expands_to_join_sql() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![sym_lit("hidings")];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str, got {:?}", args[0].node)
        };
        assert_eq!(value, "INNER JOIN hidden_stories ON hidden_stories.story_id = stories.id");
    }

    #[test]
    fn left_outer_joins_uses_left_outer_prefix() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![sym_lit("hidings")];
        lower_relation_args(&story(), &Symbol::from("left_outer_joins"), &mut args, &ctx);
        let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
            panic!("expected Str")
        };
        assert!(value.starts_with("LEFT OUTER JOIN hidden_stories ON "));
    }

    #[test]
    fn joins_unknown_assoc_left_untouched() {
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let mut args = vec![sym_lit("taggings")];
        lower_relation_args(&story(), &Symbol::from("joins"), &mut args, &ctx);
        assert!(matches!(
            &*args[0].node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "taggings"
        ));
    }

    #[test]
    fn where_belongs_to_key_renames_and_narrows_var_to_id() {
        // HiddenStory scope `by`: where(user: user) → where(user_id: user && user.id)
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let user_var = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("user") });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("user"), user_var)], kwargs: true },
        )];
        lower_relation_args(
            &ClassId(Symbol::from("HiddenStory")),
            &Symbol::from("where"),
            &mut args,
            &ctx,
        );
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (k, v) = &entries[0];
        assert!(matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "user_id"
        ));
        let ExprNode::BoolOp { op: BoolOpKind::And, left, right, .. } = &*v.node else {
            panic!("expected `user && user.id`, got {:?}", v.node)
        };
        assert!(matches!(&*left.node, ExprNode::Var { name, .. } if name.as_str() == "user"));
        assert!(matches!(
            &*right.node,
            ExprNode::Send { method, .. } if method.as_str() == "id"
        ));
    }

    #[test]
    fn where_belongs_to_key_with_nil_value_renames_only() {
        // where(user: nil) → where(user_id: nil) — `user_id IS NULL`.
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let nil = Expr::new(span(), ExprNode::Lit { value: Literal::Nil });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("user"), nil)], kwargs: true },
        )];
        lower_relation_args(
            &ClassId(Symbol::from("HiddenStory")),
            &Symbol::from("where"),
            &mut args,
            &ctx,
        );
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (k, v) = &entries[0];
        assert!(matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "user_id"
        ));
        assert!(is_nil(v));
    }

    #[test]
    fn where_non_assoc_key_untouched() {
        // where(id: x) on Story — `id` is no association; nothing changes.
        let (scopes, models, assocs) = (ScopeRegistry::new(), HashSet::new(), assoc_fixture());
        let ctx = ctx_with(&scopes, &models, &assocs);
        let x = Expr::new(span(), ExprNode::Var { id: VarId(1), name: Symbol::from("x") });
        let mut args = vec![Expr::new(
            span(),
            ExprNode::Hash { entries: vec![(sym_lit("id"), x)], kwargs: true },
        )];
        lower_relation_args(&story(), &Symbol::from("where"), &mut args, &ctx);
        let ExprNode::Hash { entries, .. } = &*args[0].node else { panic!("expected Hash") };
        let (k, v) = &entries[0];
        assert!(matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "id"
        ));
        assert!(matches!(&*v.node, ExprNode::Var { .. }));
    }

    // ---- build_assoc_registry: has_many :through ----------------------

    fn ingest(src: &str, path: &str) -> crate::dialect::Model {
        crate::ingest::ingest_model(src.as_bytes(), path, &crate::schema::Schema::default())
            .expect("ingest")
            .expect("model")
    }

    #[test]
    fn registry_resolves_has_many_through_join_tails() {
        // Tag.joins(:stories) — owner-side two-hop tail through taggings.
        let tag = ingest(
            "class Tag < ApplicationRecord\n  has_many :taggings\n  has_many :stories, through: :taggings\nend\n",
            "app/models/tag.rb",
        );
        let tagging = ingest(
            "class Tagging < ApplicationRecord\n  belongs_to :tag\n  belongs_to :story\nend\n",
            "app/models/tagging.rb",
        );
        let reg = build_assoc_registry(&[tag, tagging]);
        assert_eq!(
            reg.join_tail(&ClassId(Symbol::from("Tag")), &Symbol::from("stories")),
            Some(
                &"taggings ON taggings.tag_id = tags.id \
                   INNER JOIN stories ON stories.id = taggings.story_id"
                    .to_string()
            )
        );
    }

    #[test]
    fn registry_through_source_rename_resolves_by_target_class() {
        // `has_many :upvoted_stories, through: :votes, source: :story` —
        // ingest folds `source:` into the target class (Story); the through
        // model's `belongs_to :story` supplies the source fk.
        let user = ingest(
            "class User < ApplicationRecord\n  has_many :votes\n  has_many :upvoted_stories, through: :votes, source: :story\nend\n",
            "app/models/user.rb",
        );
        let vote = ingest(
            "class Vote < ApplicationRecord\n  belongs_to :user\n  belongs_to :story\nend\n",
            "app/models/vote.rb",
        );
        let reg = build_assoc_registry(&[user, vote]);
        assert_eq!(
            reg.join_tail(&ClassId(Symbol::from("User")), &Symbol::from("upvoted_stories")),
            Some(
                &"votes ON votes.user_id = users.id \
                   INNER JOIN stories ON stories.id = votes.story_id"
                    .to_string()
            )
        );
    }

    #[test]
    fn registry_skips_unresolvable_through() {
        // Through model absent from the set → no tail; joins(:stories)
        // stays a visible runtime symbol, not a guessed JOIN.
        let tag = ingest(
            "class Tag < ApplicationRecord\n  has_many :taggings\n  has_many :stories, through: :taggings\nend\n",
            "app/models/tag.rb",
        );
        let reg = build_assoc_registry(&[tag]);
        assert!(reg.join_tail(&ClassId(Symbol::from("Tag")), &Symbol::from("stories")).is_none());
    }
}
