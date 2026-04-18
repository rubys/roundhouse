//! Type inference for Roundhouse IR.
//!
//! MVP scope: annotate expression nodes whose types are derivable from the
//! receiver + method name against a table of known Rails / Ruby method
//! signatures. Unknown expressions get `Ty::Var(0)` as a placeholder; the
//! analyzer never fails, it just produces partial information.
//!
//! What's deliberately out of scope for this pass:
//! - Unification across branches (if/case merging types)
//! - Row-polymorphic parameter types
//! - Effect inference (separate pass)
//! - Generic instantiation beyond `Array<Post>` etc.
//! - Instance method dispatch on ivars/locals whose types aren't trivially known
//!
//! Each of those comes when a fixture forces it.

use std::collections::{BTreeSet, HashMap};

use crate::App;
use crate::dialect::{Action, Filter, FilterKind, RenderTarget};
use crate::effect::{Effect, EffectSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TableRef, TyVar};
use crate::span::Span;
use crate::ty::{Row, Ty};

pub struct Analyzer {
    classes: HashMap<ClassId, ClassInfo>,
}

/// Recursion context — what `self` is here, what locals are in scope, etc.
/// Grows as more semantic features land.
#[derive(Clone, Default)]
struct Ctx {
    self_ty: Option<Ty>,
    /// Ivar bindings observed as a `Seq` walks its statements in order.
    /// `@post = Post.find(...)` in stmt 1 lets `@post.destroy` in stmt 2
    /// dispatch correctly.
    ivar_bindings: HashMap<Symbol, Ty>,
    /// Local-variable bindings in the current scope: let-bound names,
    /// assignments accumulated through a `Seq`, and block parameters
    /// seeded from a receiver-aware dispatch. Scope threading happens
    /// by cloning Ctx when entering a new local scope (Lambda body,
    /// Let body, Seq walk).
    local_bindings: HashMap<Symbol, Ty>,
}

#[derive(Default)]
struct ClassInfo {
    /// If this class maps to a database table, which one. Used by effect
    /// inference to attach `DbRead(table)` / `DbWrite(table)` to AR methods.
    table: Option<TableRef>,
    /// Instance-state shape (columns + attr_accessor).
    attributes: Row,
    /// Methods callable on the class itself: `Post.all`, `Post.find(id)`.
    class_methods: HashMap<Symbol, Ty>,
    /// Methods callable on an instance: `post.title`, `post.destroy`.
    /// Seeded from `attributes`; instance methods on `Model.methods` will
    /// land here once we need them.
    instance_methods: HashMap<Symbol, Ty>,
}

impl Analyzer {
    pub fn new(app: &App) -> Self {
        let mut classes: HashMap<ClassId, ClassInfo> = HashMap::new();

        for model in &app.models {
            let self_ty = Ty::Class { id: model.name.clone(), args: vec![] };
            let array_of_self =
                Ty::Array { elem: Box::new(self_ty.clone()) };

            let mut cls = ClassInfo::default();
            cls.table = Some(model.table.clone());
            cls.attributes = model.attributes.clone();

            // Minimal ActiveRecord class-method signatures. Grow this table
            // as fixtures demand (`create`, `create!`, `find_by`, etc.).
            cls.class_methods.insert(Symbol::from("all"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("find"), self_ty.clone());
            cls.class_methods.insert(Symbol::from("find_by"),
                Ty::Union { variants: vec![self_ty.clone(), Ty::Nil] });
            cls.class_methods.insert(Symbol::from("first"),
                Ty::Union { variants: vec![self_ty.clone(), Ty::Nil] });
            cls.class_methods.insert(Symbol::from("last"),
                Ty::Union { variants: vec![self_ty.clone(), Ty::Nil] });
            cls.class_methods.insert(Symbol::from("where"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("limit"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("order"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("offset"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("includes"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("preload"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("joins"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("distinct"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("group"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("having"), array_of_self.clone());
            cls.class_methods.insert(Symbol::from("count"), Ty::Int);
            cls.class_methods.insert(Symbol::from("exists?"), Ty::Bool);
            cls.class_methods.insert(Symbol::from("new"), self_ty.clone());
            cls.class_methods.insert(Symbol::from("create"), self_ty.clone());

            // Instance methods from schema-derived attributes.
            for (name, ty) in &model.attributes.fields {
                cls.instance_methods.insert(name.clone(), ty.clone());
            }
            // Core AR instance methods every model gets. Return types match
            // Rails: mutation methods return Bool (non-bang) or Self (bang +
            // lifecycle). Predicates return Bool. Shape-only surface —
            // `errors` lands with the rest of the ErrorCollection dialect
            // in the wider registry expansion (#44).
            cls.instance_methods.insert(Symbol::from("save"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("save!"), self_ty.clone());
            cls.instance_methods.insert(Symbol::from("update"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("update!"), self_ty.clone());
            cls.instance_methods.insert(Symbol::from("destroy"), self_ty.clone());
            cls.instance_methods.insert(Symbol::from("destroy!"), self_ty.clone());
            cls.instance_methods.insert(Symbol::from("delete"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("touch"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("reload"), self_ty.clone());
            cls.instance_methods.insert(Symbol::from("valid?"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("invalid?"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("persisted?"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("new_record?"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("destroyed?"), Ty::Bool);
            cls.instance_methods.insert(Symbol::from("changed?"), Ty::Bool);
            cls.instance_methods.insert(
                Symbol::from("attributes"),
                Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Str) },
            );
            cls.instance_methods.insert(
                Symbol::from("errors"),
                Ty::Class {
                    id: ClassId(Symbol::from("ActiveModel::Errors")),
                    args: vec![],
                },
            );
            // Associations as instance methods (return types derived from cardinality).
            for assoc in model.associations() {
                use crate::dialect::Association;
                match assoc {
                    Association::BelongsTo { name, target, .. } => {
                        cls.instance_methods.insert(
                            name.clone(),
                            Ty::Union {
                                variants: vec![
                                    Ty::Class { id: target.clone(), args: vec![] },
                                    Ty::Nil,
                                ],
                            },
                        );
                    }
                    Association::HasOne { name, target, .. } => {
                        cls.instance_methods.insert(
                            name.clone(),
                            Ty::Union {
                                variants: vec![
                                    Ty::Class { id: target.clone(), args: vec![] },
                                    Ty::Nil,
                                ],
                            },
                        );
                    }
                    Association::HasMany { name, target, .. }
                    | Association::HasAndBelongsToMany { name, target, .. } => {
                        cls.instance_methods.insert(
                            name.clone(),
                            Ty::Array {
                                elem: Box::new(Ty::Class { id: target.clone(), args: vec![] }),
                            },
                        );
                    }
                }
            }

            classes.insert(model.name.clone(), cls);
        }

        // ActiveModel::Errors — the collection returned by `model.errors`.
        // Supports count/[]/any?/each and flows a Error instance to blocks.
        let error_ty = Ty::Class {
            id: ClassId(Symbol::from("ActiveModel::Error")),
            args: vec![],
        };
        let mut errors_cls = ClassInfo::default();
        errors_cls
            .instance_methods
            .insert(Symbol::from("count"), Ty::Int);
        errors_cls
            .instance_methods
            .insert(Symbol::from("size"), Ty::Int);
        errors_cls
            .instance_methods
            .insert(Symbol::from("any?"), Ty::Bool);
        errors_cls
            .instance_methods
            .insert(Symbol::from("none?"), Ty::Bool);
        errors_cls
            .instance_methods
            .insert(Symbol::from("empty?"), Ty::Bool);
        errors_cls
            .instance_methods
            .insert(Symbol::from("include?"), Ty::Bool);
        errors_cls.instance_methods.insert(
            Symbol::from("full_messages"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        // `errors[:title]` returns an Array<String> of messages for that attribute.
        errors_cls.instance_methods.insert(
            Symbol::from("[]"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        errors_cls.instance_methods.insert(
            Symbol::from("messages_for"),
            Ty::Array { elem: Box::new(Ty::Str) },
        );
        // `.each` yields an Error — registered via block_params_for below.
        errors_cls
            .instance_methods
            .insert(Symbol::from("each"), error_ty.clone());
        classes.insert(
            ClassId(Symbol::from("ActiveModel::Errors")),
            errors_cls,
        );

        // Individual Error with its Rails API.
        let mut error_cls = ClassInfo::default();
        error_cls
            .instance_methods
            .insert(Symbol::from("full_message"), Ty::Str);
        error_cls
            .instance_methods
            .insert(Symbol::from("message"), Ty::Str);
        error_cls
            .instance_methods
            .insert(Symbol::from("attribute"), Ty::Sym);
        error_cls
            .instance_methods
            .insert(Symbol::from("type"), Ty::Sym);
        classes.insert(
            ClassId(Symbol::from("ActiveModel::Error")),
            error_cls,
        );

        // Hardcoded ApplicationController-ish surface. Real inheritance chains
        // and per-controller overrides land when a fixture forces them.
        let mut app_ctrl = ClassInfo::default();
        let params_ty = Ty::Hash {
            key: Box::new(Ty::Sym),
            value: Box::new(Ty::Str),
        };
        app_ctrl.class_methods.insert(Symbol::from("params"), params_ty);
        app_ctrl.class_methods.insert(Symbol::from("session"),
            Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) });
        app_ctrl.class_methods.insert(Symbol::from("render"), Ty::Nil);
        app_ctrl.class_methods.insert(Symbol::from("redirect_to"), Ty::Nil);
        app_ctrl.class_methods.insert(Symbol::from("head"), Ty::Nil);
        classes.insert(ClassId(Symbol::from("ApplicationController")), app_ctrl);

        Self { classes }
    }

    /// Walk the app, annotating every expression's `ty` field, then
    /// populating the owning construct's `effects` by visiting the typed tree.
    pub fn analyze(&self, app: &mut App) {
        // Controller→view ivar channel: as each action is analyzed, we harvest
        // the ivars it sets and key them by the view that action renders.
        // When we reach the view pass below, the view's Ctx is seeded from
        // this map so `@article.title` in `articles/show.html.erb` types
        // against the `@article` bound in `ArticlesController#show`.
        let mut action_ivars_by_view: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        for controller in &mut app.controllers {
            let ctx = Ctx {
                self_ty: Some(Ty::Class {
                    id: controller
                        .parent
                        .clone()
                        .unwrap_or_else(|| ClassId(Symbol::from("ApplicationController"))),
                    args: vec![],
                }),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
            };
            let ctrl_name = controller.name.clone();

            // Snapshot every `before_action` on this controller once, so the
            // two-pass analysis below can consult the list without re-borrow.
            let before_filters: Vec<Filter> = controller
                .filters()
                .filter(|f| matches!(f.kind, FilterKind::Before))
                .cloned()
                .collect();

            // Pass A: analyze every action body once with no seed. Required
            // before we can harvest each action's produced ivar bindings —
            // the callback targets (`set_article`) are themselves actions.
            for action in controller.actions_mut() {
                self.analyze_expr(&mut action.body, &ctx);
                action.effects = self.collect_effects(&action.body, &ctx);
            }

            // Snapshot each action's ivar_bindings. Used both to resolve
            // before_action targets (Pass B) and to seed view Ctx (below).
            let action_bindings: HashMap<Symbol, HashMap<Symbol, Ty>> = controller
                .actions()
                .map(|a| {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments(&a.body, &mut ivars);
                    (a.name.clone(), ivars)
                })
                .collect();

            // Pass B: re-analyze actions affected by a before_action with the
            // target's bindings pre-seeded into Ctx. Rails' `before_action`
            // runs before the action body, so any ivar the filter sets is in
            // scope for the whole body. Idempotent analyze means two passes
            // produce consistent types; cost is negligible for real
            // controllers.
            if !before_filters.is_empty() {
                for action in controller.actions_mut() {
                    let seed = merged_before_seed(&before_filters, &action.name, &action_bindings);
                    if !seed.is_empty() {
                        let inner_ctx = Ctx {
                            self_ty: ctx.self_ty.clone(),
                            ivar_bindings: seed,
                            local_bindings: HashMap::new(),
                        };
                        self.analyze_expr(&mut action.body, &inner_ctx);
                        action.effects = self.collect_effects(&action.body, &inner_ctx);
                    }
                }
            }

            // Build the per-view ivar map. Each view gets the action's own
            // assignments *plus* any before_action contribution (which isn't
            // syntactically present in the action body).
            for action in controller.actions() {
                if let Some(view_name) = view_name_for_action(&ctrl_name, action) {
                    let mut ivars = HashMap::new();
                    extract_ivar_assignments(&action.body, &mut ivars);
                    for filter in &before_filters {
                        if before_filter_applies(filter, &action.name) {
                            if let Some(fivars) = action_bindings.get(&filter.target) {
                                for (k, v) in fivars {
                                    ivars.entry(k.clone()).or_insert_with(|| v.clone());
                                }
                            }
                        }
                    }
                    action_ivars_by_view.insert(view_name, ivars);
                }
            }
        }
        for model in &mut app.models {
            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                ivar_bindings: HashMap::new(),
                local_bindings: HashMap::new(),
            };
            for scope in model.scopes_mut() {
                self.analyze_expr(&mut scope.body, &class_ctx);
            }
            for method in model.methods_mut() {
                self.analyze_expr(&mut method.body, &class_ctx);
                method.effects = self.collect_effects(&method.body, &class_ctx);
            }
        }
        // Partial-locals channel: we need action/top-level views analyzed first
        // so their expression types are known at each `render` call site. We
        // then harvest the locals each render passes to the target partial,
        // keying by the partial's view name, and analyze partials with that
        // seed. Nested partial-of-partial isn't handled here (would need a
        // fixpoint); real-blog's dependency graph is shallow enough to skip.
        let mut partial_locals_by_name: HashMap<Symbol, HashMap<Symbol, Ty>> = HashMap::new();

        // Phase 3a: non-partial views (action views + layouts). Analyze with
        // the controller→view ivar seed, then walk the body to record every
        // `render` call's effect on partial_locals_by_name.
        for view in &mut app.views {
            if is_partial_view_name(&view.name) {
                continue;
            }
            let mut view_ctx = Ctx::default();
            if let Some(ivars) = action_ivars_by_view.get(&view.name) {
                view_ctx.ivar_bindings = ivars.clone();
            }
            self.analyze_expr(&mut view.body, &view_ctx);
            extract_partial_render_sites(&view.body, &view.name, &mut partial_locals_by_name);
        }

        // Phase 3b: partials. Seed local_bindings from the map built above,
        // then analyze.
        for view in &mut app.views {
            if !is_partial_view_name(&view.name) {
                continue;
            }
            let mut view_ctx = Ctx::default();
            if let Some(locals) = partial_locals_by_name.get(&view.name) {
                view_ctx.local_bindings = locals.clone();
            }
            self.analyze_expr(&mut view.body, &view_ctx);
        }
    }

    fn collect_effects(&self, expr: &Expr, ctx: &Ctx) -> EffectSet {
        let mut set = BTreeSet::new();
        self.visit_effects(expr, ctx, &mut set);
        EffectSet { effects: set }
    }

    fn visit_effects(&self, expr: &Expr, ctx: &Ctx, out: &mut BTreeSet<Effect>) {
        match &*expr.node {
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. } => {}

            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    self.visit_effects(k, ctx, out);
                    self.visit_effects(v, ctx, out);
                }
            }

            ExprNode::Array { elements, .. } => {
                for e in elements {
                    self.visit_effects(e, ctx, out);
                }
            }

            ExprNode::StringInterp { parts } => {
                for p in parts {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.visit_effects(expr, ctx, out);
                    }
                }
            }

            ExprNode::BoolOp { left, right, .. } => {
                self.visit_effects(left, ctx, out);
                self.visit_effects(right, ctx, out);
            }

            ExprNode::RescueModifier { expr, fallback } => {
                self.visit_effects(expr, ctx, out);
                self.visit_effects(fallback, ctx, out);
            }

            ExprNode::Let { value, body, .. } => {
                self.visit_effects(value, ctx, out);
                self.visit_effects(body, ctx, out);
            }
            ExprNode::Lambda { body, .. } => {
                // Lambda creation is pure; only invocation has effects. A
                // proper treatment requires first-class Fn types. Skip for now.
                self.visit_effects(body, ctx, out);
            }
            ExprNode::Apply { fun, args, block } => {
                self.visit_effects(fun, ctx, out);
                for a in args { self.visit_effects(a, ctx, out); }
                if let Some(b) = block { self.visit_effects(b, ctx, out); }
            }
            ExprNode::Send { recv, method, args, block, .. } => {
                let recv_ty = match recv {
                    Some(r) => {
                        self.visit_effects(r, ctx, out);
                        r.ty.clone()
                    }
                    None => ctx.self_ty.clone(),
                };
                if let Some(ty) = recv_ty {
                    self.contribute_send_effect(&ty, method, out);
                }
                for a in args { self.visit_effects(a, ctx, out); }
                if let Some(b) = block { self.visit_effects(b, ctx, out); }
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                self.visit_effects(cond, ctx, out);
                self.visit_effects(then_branch, ctx, out);
                self.visit_effects(else_branch, ctx, out);
            }
            ExprNode::Case { scrutinee, arms } => {
                self.visit_effects(scrutinee, ctx, out);
                for arm in arms {
                    if let Some(g) = &arm.guard { self.visit_effects(g, ctx, out); }
                    self.visit_effects(&arm.body, ctx, out);
                }
            }
            ExprNode::Seq { exprs } => {
                for e in exprs { self.visit_effects(e, ctx, out); }
            }
            ExprNode::Assign { target, value } => {
                self.visit_effects(value, ctx, out);
                if let LValue::Attr { recv, .. } = target {
                    self.visit_effects(recv, ctx, out);
                }
                if let LValue::Index { recv, index } = target {
                    self.visit_effects(recv, ctx, out);
                    self.visit_effects(index, ctx, out);
                }
            }
            ExprNode::Yield { args } => {
                for a in args { self.visit_effects(a, ctx, out); }
            }
            ExprNode::Raise { value } => {
                self.visit_effects(value, ctx, out);
                // Could record a Raises effect here once we track exception
                // class hierarchies. Skip for now.
            }
        }
    }

    fn contribute_send_effect(&self, recv_ty: &Ty, method: &Symbol, out: &mut BTreeSet<Effect>) {
        let Ty::Class { id, .. } = recv_ty else { return };
        let Some(cls) = self.classes.get(id) else { return };

        // AR methods on model classes: DbRead / DbWrite against the bound table.
        if let Some(table) = &cls.table {
            let m = method.as_str();
            if is_db_read_method(m) {
                out.insert(Effect::DbRead { table: table.clone() });
            } else if is_db_write_method(m) {
                out.insert(Effect::DbWrite { table: table.clone() });
            }
        }

        // Controller-side IO effects.
        if id.0.as_str() == "ApplicationController" {
            match method.as_str() {
                "render" | "redirect_to" | "head" => {
                    out.insert(Effect::Io);
                }
                _ => {}
            }
        }
    }

    fn analyze_expr(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
        let ty = self.compute(expr, ctx);
        expr.ty = Some(ty.clone());
        ty
    }

    fn compute(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
        match &mut *expr.node {
            ExprNode::Lit { value } => lit_ty(value),

            ExprNode::Const { path } => {
                // `Post` as an expression refers to the class.
                let name = path.last().cloned().unwrap_or_else(|| Symbol::from("?"));
                Ty::Class { id: ClassId(name), args: vec![] }
            }

            ExprNode::Var { name, .. } => ctx
                .local_bindings
                .get(name)
                .cloned()
                .unwrap_or_else(unknown),

            ExprNode::Ivar { name } => {
                ctx.ivar_bindings.get(name).cloned().unwrap_or_else(unknown)
            }

            ExprNode::Hash { entries, .. } => {
                let mut key_ty: Option<Ty> = None;
                let mut value_ty: Option<Ty> = None;
                for (k, v) in entries.iter_mut() {
                    let kt = self.analyze_expr(k, ctx);
                    let vt = self.analyze_expr(v, ctx);
                    key_ty = Some(match key_ty.take() {
                        Some(prev) => union_of(prev, kt),
                        None => kt,
                    });
                    value_ty = Some(match value_ty.take() {
                        Some(prev) => union_of(prev, vt),
                        None => vt,
                    });
                }
                Ty::Hash {
                    key: Box::new(key_ty.unwrap_or_else(unknown)),
                    value: Box::new(value_ty.unwrap_or_else(unknown)),
                }
            }

            ExprNode::Array { elements, .. } => {
                let mut elem_ty: Option<Ty> = None;
                for e in elements.iter_mut() {
                    let et = self.analyze_expr(e, ctx);
                    elem_ty = Some(match elem_ty.take() {
                        Some(prev) => union_of(prev, et),
                        None => et,
                    });
                }
                Ty::Array { elem: Box::new(elem_ty.unwrap_or_else(unknown)) }
            }

            ExprNode::StringInterp { parts } => {
                for p in parts.iter_mut() {
                    if let crate::expr::InterpPart::Expr { expr } = p {
                        self.analyze_expr(expr, ctx);
                    }
                }
                Ty::Str
            }

            ExprNode::BoolOp { left, right, .. } => {
                let lt = self.analyze_expr(left, ctx);
                let rt = self.analyze_expr(right, ctx);
                // Short-circuit: the result is either left (if truthy) or
                // right — a union of the two operand types.
                union_of(lt, rt)
            }

            ExprNode::RescueModifier { expr, fallback } => {
                let et = self.analyze_expr(expr, ctx);
                let ft = self.analyze_expr(fallback, ctx);
                union_of(et, ft)
            }

            ExprNode::Let { name, value, body, .. } => {
                let v_ty = self.analyze_expr(value, ctx);
                let mut inner = ctx.clone();
                inner.local_bindings.insert(name.clone(), v_ty);
                self.analyze_expr(body, &inner)
            }

            ExprNode::Lambda { body, .. } => {
                self.analyze_expr(body, ctx);
                unknown() // Fn type synthesis is future work
            }

            ExprNode::Apply { fun, args, block } => {
                self.analyze_expr(fun, ctx);
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                if let Some(b) = block { self.analyze_expr(b, ctx); }
                unknown()
            }

            ExprNode::Send { recv, method, args, block, .. } => {
                // Bare-name implicit-self Send (no receiver, no args, no
                // block) resolves to a local binding when one exists. Ruby
                // parses `x` as `self.x()` when `x` wasn't assigned earlier
                // in scope; partials receive locals at render time (not via
                // syntactic assignment), so we disambiguate here instead of
                // at ingest. Block params and let-bindings end up on the
                // same code path.
                if recv.is_none() && args.is_empty() && block.is_none() {
                    if let Some(ty) = ctx.local_bindings.get(method) {
                        return ty.clone();
                    }
                }
                let recv_ty = match recv.as_mut() {
                    Some(r) => Some(self.analyze_expr(r, ctx)),
                    None => ctx.self_ty.clone(),
                };
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                if let Some(b) = block {
                    let block_ctx = self.block_ctx_for(ctx, recv_ty.as_ref(), method, b);
                    self.analyze_expr(b, &block_ctx);
                }
                self.dispatch(recv_ty.as_ref(), method)
            }

            ExprNode::If { cond, then_branch, else_branch } => {
                self.analyze_expr(cond, ctx);
                let t = self.analyze_expr(then_branch, ctx);
                let e = self.analyze_expr(else_branch, ctx);
                union_of(t, e)
            }

            ExprNode::Case { scrutinee, arms } => {
                self.analyze_expr(scrutinee, ctx);
                let mut branch_tys = Vec::new();
                for arm in arms.iter_mut() {
                    if let Some(g) = &mut arm.guard { self.analyze_expr(g, ctx); }
                    branch_tys.push(self.analyze_expr(&mut arm.body, ctx));
                }
                union_many(branch_tys)
            }

            ExprNode::Seq { exprs } => {
                // Within a Seq, walk statements in order and thread
                // bindings forward: `@post = Post.find(...)` in stmt i
                // lets stmt i+1 resolve `@post`. Same for local vars —
                // `x = post.title` binds `x` for subsequent statements.
                let mut local_ctx = ctx.clone();
                let mut last = Ty::Nil;
                for e in exprs.iter_mut() {
                    last = self.analyze_expr(e, &local_ctx);
                    if let ExprNode::Assign { target, .. } = &*e.node {
                        if let Some(ty) = e.ty.clone() {
                            match target {
                                LValue::Ivar { name } => {
                                    local_ctx.ivar_bindings.insert(name.clone(), ty);
                                }
                                LValue::Var { name, .. } => {
                                    local_ctx.local_bindings.insert(name.clone(), ty);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                last
            }

            ExprNode::Assign { target, value } => {
                let value_ty = self.analyze_expr(value, ctx);
                if let LValue::Attr { recv, .. } = target {
                    self.analyze_expr(recv, ctx);
                }
                if let LValue::Index { recv, index } = target {
                    self.analyze_expr(recv, ctx);
                    self.analyze_expr(index, ctx);
                }
                value_ty
            }

            ExprNode::Yield { args } => {
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                unknown()
            }

            ExprNode::Raise { value } => {
                self.analyze_expr(value, ctx);
                Ty::Nil
            }
        }
    }

    /// Build the Ctx used to analyze a block passed to `recv.method(...) { |p1, p2| ... }`.
    /// Seeds the block's local_bindings with parameter types derived from the receiver
    /// and method (e.g. `array.each { |x| }` binds `x` to the array's element type).
    fn block_ctx_for(
        &self,
        outer: &Ctx,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        block: &Expr,
    ) -> Ctx {
        let mut new_ctx = outer.clone();
        let ExprNode::Lambda { params, .. } = &*block.node else {
            return new_ctx;
        };
        let Some(param_tys) = self.block_params_for(recv_ty, method) else {
            return new_ctx;
        };
        for (name, ty) in params.iter().zip(param_tys.iter()) {
            new_ctx.local_bindings.insert(name.clone(), ty.clone());
        }
        new_ctx
    }

    /// Per-param types a block yields, given the receiver type and method.
    /// `None` means "no binding info available" — params stay unknown.
    fn block_params_for(&self, recv_ty: Option<&Ty>, method: &Symbol) -> Option<Vec<Ty>> {
        let recv_ty = recv_ty?;
        match recv_ty {
            Ty::Array { elem } => match method.as_str() {
                "each" | "map" | "collect" | "select" | "filter" | "reject"
                | "find" | "detect" | "sort_by" | "group_by" | "min_by" | "max_by"
                | "any?" | "all?" | "none?" | "one?" => Some(vec![(**elem).clone()]),
                "each_with_index" => Some(vec![(**elem).clone(), Ty::Int]),
                _ => None,
            },
            Ty::Hash { key, value } => match method.as_str() {
                "each" | "each_pair" | "map" | "collect" | "select" | "filter"
                | "reject" | "any?" | "all?" | "none?" => {
                    Some(vec![(**key).clone(), (**value).clone()])
                }
                _ => None,
            },
            // ActiveModel::Errors iteration yields an Error to the block.
            Ty::Class { id, .. } if id.0.as_str() == "ActiveModel::Errors" => {
                match method.as_str() {
                    "each" | "map" | "collect" | "select" | "filter" | "reject"
                    | "any?" | "all?" | "none?" => Some(vec![Ty::Class {
                        id: ClassId(Symbol::from("ActiveModel::Error")),
                        args: vec![],
                    }]),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn dispatch(&self, recv_ty: Option<&Ty>, method: &Symbol) -> Ty {
        match recv_ty {
            None => unknown(),
            Some(Ty::Class { id, .. }) => {
                if let Some(cls) = self.classes.get(id) {
                    if let Some(ty) = cls.class_methods.get(method) {
                        return ty.clone();
                    }
                    if let Some(ty) = cls.instance_methods.get(method) {
                        return ty.clone();
                    }
                }
                unknown()
            }
            Some(Ty::Array { elem }) => array_method(method, elem),
            Some(Ty::Hash { value, .. }) => hash_method(method, value),
            Some(Ty::Str) => str_method(method),
            Some(Ty::Int) => int_method(method),
            // Union dispatch: try each concrete (non-Nil, non-Var) variant
            // and union the resolved results. Covers the common
            // `T | Nil` pattern (`find_by`, `params[:k]`, `.find` on
            // relation) where the method is valid on `T` and the Nil case
            // is handled elsewhere at run time.
            Some(Ty::Union { variants }) => {
                let mut resolved: Vec<Ty> = Vec::new();
                for v in variants {
                    if matches!(v, Ty::Nil | Ty::Var { .. }) {
                        continue;
                    }
                    let r = self.dispatch(Some(v), method);
                    if !matches!(r, Ty::Var { .. }) {
                        resolved.push(r);
                    }
                }
                match resolved.len() {
                    0 => unknown(),
                    1 => resolved.into_iter().next().unwrap(),
                    _ => union_many(resolved),
                }
            }
            _ => unknown(),
        }
    }
}

// Literal / primitive types ---------------------------------------------

fn lit_ty(lit: &Literal) -> Ty {
    match lit {
        Literal::Nil => Ty::Nil,
        Literal::Bool { .. } => Ty::Bool,
        Literal::Int { .. } => Ty::Int,
        Literal::Float { .. } => Ty::Float,
        Literal::Str { .. } => Ty::Str,
        Literal::Sym { .. } => Ty::Sym,
    }
}

fn array_method(method: &Symbol, elem: &Ty) -> Ty {
    // AR-specific dispatches go FIRST so they win over the generic
    // array methods that share a name (`find` on a relation raises, so
    // it returns Class; on a plain Array it returns `Union<elem, Nil>`).
    if matches!(elem, Ty::Class { .. }) {
        match method.as_str() {
            // Relation chain methods preserve Array<Self>.
            "where" | "order" | "limit" | "offset" | "includes" | "preload"
            | "joins" | "distinct" | "group" | "having" => {
                return Ty::Array { elem: Box::new(elem.clone()) };
            }
            // CollectionProxy constructors return an element instance.
            "build" | "create" | "create!" | "find" | "find!" => {
                return elem.clone();
            }
            _ => {}
        }
    }
    match method.as_str() {
        "length" | "size" | "count" => Ty::Int,
        "first" | "last" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "[]" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "each" | "map" | "collect" | "select" | "filter" | "reject"
        | "sort" | "sort_by" | "reverse" | "compact" | "flatten" | "uniq" => {
            Ty::Array { elem: Box::new(elem.clone()) }
        }
        "any?" | "all?" | "none?" | "one?" | "empty?" | "include?" => Ty::Bool,
        "find" | "detect" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        _ => unknown(),
    }
}

fn hash_method(method: &Symbol, value: &Ty) -> Ty {
    match method.as_str() {
        "[]" => Ty::Union { variants: vec![value.clone(), Ty::Nil] },
        "length" | "size" | "count" => Ty::Int,
        "values" => Ty::Array { elem: Box::new(value.clone()) },
        "empty?" | "any?" | "none?" | "key?" | "has_key?" | "include?" => Ty::Bool,
        "keys" => Ty::Array { elem: Box::new(Ty::Sym) },
        "fetch" => value.clone(),
        "merge" => Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(value.clone()) },
        // Rails strong-params: `params.expect(:id)` and
        // `params.expect(k: [...])` both return the coerced value (a
        // scalar or a permitted-params-hash). Approximate both as the
        // value type for now; refine when a fixture forces a richer
        // return shape.
        "expect" | "require" | "permit" => value.clone(),
        _ => unknown(),
    }
}

fn str_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "length" | "size" | "bytesize" => Ty::Int,
        "upcase" | "downcase" | "strip" | "chomp" | "chop" | "reverse" | "to_s"
        | "capitalize" | "swapcase" | "squeeze" | "dup" | "clone" => Ty::Str,
        "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "empty?" | "blank?" | "present?" | "include?" | "start_with?"
        | "end_with?" | "match?" => Ty::Bool,
        // Operators. `+` concats; `<<` mutates in place but still returns self.
        // `*` is repetition ("a" * 3). Comparisons uniformly return Bool.
        "+" | "<<" | "*" | "concat" => Ty::Str,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        _ => unknown(),
    }
}

fn int_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "to_s" => Ty::Str,
        "to_i" | "abs" | "succ" | "pred" => Ty::Int,
        "to_f" => Ty::Float,
        "zero?" | "positive?" | "negative?" | "even?" | "odd?" => Ty::Bool,
        // Arithmetic: Int op Int → Int (we approximate Int/Float mixing here;
        // refine when a fixture demands it).
        "+" | "-" | "*" | "/" | "%" | "**" | "&" | "|" | "^" | "<<" | ">>" => Ty::Int,
        "==" | "!=" | "<" | ">" | "<=" | ">=" | "<=>" | "eql?" | "equal?" => Ty::Bool,
        _ => unknown(),
    }
}

fn unknown() -> Ty {
    Ty::Var { var: TyVar(0) }
}

fn is_db_read_method(m: &str) -> bool {
    matches!(
        m,
        "all"
            | "find"
            | "find_by"
            | "find_by!"
            | "first"
            | "last"
            | "where"
            | "limit"
            | "offset"
            | "order"
            | "group"
            | "having"
            | "joins"
            | "includes"
            | "preload"
            | "select"
            | "distinct"
            | "count"
            | "exists?"
            | "pluck"
            | "pick"
            | "take"
            | "sum"
            | "average"
            | "maximum"
            | "minimum"
    )
}

fn is_db_write_method(m: &str) -> bool {
    matches!(
        m,
        "save"
            | "save!"
            | "create"
            | "create!"
            | "update"
            | "update!"
            | "update_all"
            | "destroy"
            | "destroy!"
            | "destroy_all"
            | "delete"
            | "delete_all"
            | "increment!"
            | "decrement!"
            | "touch"
            | "touch_all"
            | "insert"
            | "insert_all"
            | "upsert"
            | "upsert_all"
    )
}

/// Does `filter` apply to the action named `action_name`? Rails scopes:
/// - `only: [...]` limits to the listed actions
/// - `except: [...]` excludes the listed actions
/// - both empty → applies to all actions on the controller
fn before_filter_applies(filter: &Filter, action_name: &Symbol) -> bool {
    if !filter.only.is_empty() {
        return filter.only.contains(action_name);
    }
    if !filter.except.is_empty() {
        return !filter.except.contains(action_name);
    }
    true
}

/// Merge ivar bindings from every before_action that applies to this action,
/// looking up each filter's `target` in the pre-computed per-action bindings
/// table. Later filters overwrite earlier ones on conflicting keys —
/// matches Rails' "last-registered wins" when the same ivar is set by
/// multiple callbacks.
fn merged_before_seed(
    before_filters: &[Filter],
    action_name: &Symbol,
    action_bindings: &HashMap<Symbol, HashMap<Symbol, Ty>>,
) -> HashMap<Symbol, Ty> {
    let mut seed: HashMap<Symbol, Ty> = HashMap::new();
    for filter in before_filters {
        if before_filter_applies(filter, action_name) {
            if let Some(fivars) = action_bindings.get(&filter.target) {
                for (k, v) in fivars {
                    seed.insert(k.clone(), v.clone());
                }
            }
        }
    }
    seed
}

/// A view name identifies a partial when any path segment starts with `_`
/// (Rails convention: `app/views/articles/_article.html.erb` → view name
/// `articles/_article`).
fn is_partial_view_name(name: &Symbol) -> bool {
    name.as_str().split('/').any(|seg| seg.starts_with('_'))
}

/// Walk a view body collecting `render ...` call sites. For each recognized
/// shape, determine the target partial's view name and the locals the render
/// passes into it, merging into `out`.
///
/// Shapes recognized (matching real-blog + the common idioms):
/// - `render @collection` where `@collection` types as `Array<Class>` →
///   partial `pluralize(snake(Class))/_snake(Class)`, local `snake(Class)`.
/// - `render some_single_record` typing as `Class` → same partial path, local
///   bound to the record's type.
/// - `render "name", k1: v1, k2: v2` → partial name resolved relative to the
///   current view's directory (`articles/index` + `"form"` → `articles/_form`),
///   locals from the trailing kwarg hash.
/// - `render partial: "name", locals: { k: v }` → same resolution, locals
///   sourced from the `locals:` hash.
///
/// Call-site argument shapes outside these cases are skipped silently;
/// an unrecognized render just leaves the target partial seeded by other
/// sites (or unseeded).
fn extract_partial_render_sites(
    expr: &Expr,
    current_view: &Symbol,
    out: &mut HashMap<Symbol, HashMap<Symbol, Ty>>,
) {
    match &*expr.node {
        ExprNode::Send { recv, method, args, block, .. } => {
            // Detect the `render` call shape (no explicit receiver, or the
            // receiver is an implicit context — Rails makes both work).
            if recv.is_none() && method.as_str() == "render" {
                if let Some((partial_name, locals)) = interpret_render_call(args, current_view) {
                    let entry = out.entry(partial_name).or_default();
                    for (k, v) in locals {
                        entry.insert(k, v);
                    }
                }
            }
            if let Some(r) = recv {
                extract_partial_render_sites(r, current_view, out);
            }
            for a in args {
                extract_partial_render_sites(a, current_view, out);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                extract_partial_render_sites(e, current_view, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                extract_partial_render_sites(k, current_view, out);
                extract_partial_render_sites(v, current_view, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            extract_partial_render_sites(cond, current_view, out);
            extract_partial_render_sites(then_branch, current_view, out);
            extract_partial_render_sites(else_branch, current_view, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            extract_partial_render_sites(scrutinee, current_view, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    extract_partial_render_sites(g, current_view, out);
                }
                extract_partial_render_sites(&arm.body, current_view, out);
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            extract_partial_render_sites(left, current_view, out);
            extract_partial_render_sites(right, current_view, out);
        }
        ExprNode::Let { value, body, .. } => {
            extract_partial_render_sites(value, current_view, out);
            extract_partial_render_sites(body, current_view, out);
        }
        ExprNode::Lambda { body, .. } => {
            extract_partial_render_sites(body, current_view, out);
        }
        ExprNode::Apply { fun, args, block } => {
            extract_partial_render_sites(fun, current_view, out);
            for a in args {
                extract_partial_render_sites(a, current_view, out);
            }
            if let Some(b) = block {
                extract_partial_render_sites(b, current_view, out);
            }
        }
        ExprNode::Assign { value, .. } => {
            extract_partial_render_sites(value, current_view, out);
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    extract_partial_render_sites(expr, current_view, out);
                }
            }
        }
        _ => {}
    }
}

/// Figure out the target partial name and the locals a `render(...)` call
/// passes to it. Returns `None` for shapes not yet handled.
fn interpret_render_call(
    args: &[Expr],
    current_view: &Symbol,
) -> Option<(Symbol, HashMap<Symbol, Ty>)> {
    if args.is_empty() {
        return None;
    }
    let first = &args[0];

    // Collection / single-record render: `render @articles`, `render @article.comments`,
    // `render @article` — first arg types as Array<Class> or Class.
    if let Some(ty) = first.ty.as_ref() {
        if let Some((partial, local_name, elem_ty)) = partial_from_receiver_type(ty) {
            let mut locals = HashMap::new();
            locals.insert(Symbol::from(local_name.as_str()), elem_ty);
            return Some((Symbol::from(partial.as_str()), locals));
        }
    }

    // Named partial: `render "name", k: v, k: v` or `render "name"`.
    if let ExprNode::Lit { value: Literal::Str { value: name } } = &*first.node {
        let partial = resolve_partial_path(name, current_view);
        let mut locals = HashMap::new();
        for a in &args[1..] {
            if let ExprNode::Hash { entries, .. } = &*a.node {
                for (k, v) in entries {
                    if let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node {
                        if let Some(ty) = v.ty.clone() {
                            locals.insert(key.clone(), ty);
                        }
                    }
                }
            }
        }
        return Some((Symbol::from(partial.as_str()), locals));
    }

    // Hash form: `render partial: "name", locals: { k: v }` — first arg is a Hash.
    if let ExprNode::Hash { entries, .. } = &*first.node {
        let mut partial_name: Option<String> = None;
        let mut locals: HashMap<Symbol, Ty> = HashMap::new();
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            match key.as_str() {
                "partial" => {
                    if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                        partial_name = Some(value.clone());
                    }
                }
                "locals" => {
                    if let ExprNode::Hash { entries: loc_entries, .. } = &*v.node {
                        for (lk, lv) in loc_entries {
                            if let ExprNode::Lit { value: Literal::Sym { value: loc_key } } =
                                &*lk.node
                            {
                                if let Some(ty) = lv.ty.clone() {
                                    locals.insert(loc_key.clone(), ty);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(name) = partial_name {
            let partial = resolve_partial_path(&name, current_view);
            return Some((Symbol::from(partial.as_str()), locals));
        }
    }

    None
}

/// If the receiver type implies a collection/single-record render target,
/// return (partial_view_name, local_name, element_ty). For `Array<Article>`:
/// partial `articles/_article`, local `article`, element `Article`.
fn partial_from_receiver_type(ty: &Ty) -> Option<(String, String, Ty)> {
    match ty {
        Ty::Array { elem } => match &**elem {
            Ty::Class { id, .. } => {
                let class_name = id.0.as_str();
                let local = crate::naming::snake_case(class_name);
                let folder = crate::naming::pluralize_snake(class_name);
                Some((format!("{folder}/_{local}"), local, (**elem).clone()))
            }
            _ => None,
        },
        Ty::Class { id, .. } => {
            let class_name = id.0.as_str();
            let local = crate::naming::snake_case(class_name);
            let folder = crate::naming::pluralize_snake(class_name);
            Some((format!("{folder}/_{local}"), local, ty.clone()))
        }
        _ => None,
    }
}

/// Resolve a partial name relative to the current view's directory.
/// `"form"` in `articles/index` → `articles/_form`; `"shared/nav"` (absolute,
/// contains `/`) → `shared/_nav`.
fn resolve_partial_path(name: &str, current_view: &Symbol) -> String {
    if let Some(idx) = name.rfind('/') {
        let (dir, file) = name.split_at(idx + 1);
        format!("{dir}_{file}")
    } else {
        let current = current_view.as_str();
        match current.rfind('/') {
            Some(idx) => format!("{}_{}", &current[..=idx], name),
            None => format!("_{name}"),
        }
    }
}

/// Convert a controller class name into the view-path prefix.
/// `ArticlesController` → `articles`. Strip the `Controller` suffix, then
/// snake_case what remains. Namespaced controllers (`Admin::UsersController`)
/// are handled by the current snake_case rule producing `admin::users`; when
/// a fixture forces namespaced views, we'll fix the rule to emit `/` instead.
fn controller_view_prefix(class_id: &ClassId) -> String {
    let name = class_id.0.as_str();
    let stripped = name.strip_suffix("Controller").unwrap_or(name);
    crate::naming::snake_case(stripped)
}

/// Determine which view path an action's RenderTarget names — `None` if
/// the action doesn't render a template (redirect, JSON, head).
fn view_name_for_action(controller: &ClassId, action: &Action) -> Option<Symbol> {
    let prefix = controller_view_prefix(controller);
    match &action.renders {
        RenderTarget::Inferred => {
            Some(Symbol::from(format!("{}/{}", prefix, action.name.as_str())))
        }
        RenderTarget::Template { name, .. } => {
            let n = name.as_str();
            if n.contains('/') {
                Some(Symbol::from(n))
            } else {
                Some(Symbol::from(format!("{}/{}", prefix, n)))
            }
        }
        RenderTarget::Redirect { .. }
        | RenderTarget::Json { .. }
        | RenderTarget::Head { .. } => None,
    }
}

/// Walk an action body collecting every `@ivar = expr` assignment into
/// `out`, keyed by ivar name → expression type. Used to seed the view's
/// Ctx so that `@post.title` in the template resolves against the action
/// that renders it.
///
/// Walks through branching constructs (If, RescueModifier) so ivars set
/// conditionally still show up. Deliberately does NOT walk into blocks
/// (Lambda bodies): ivars assigned inside iteration are run-time per-element
/// state, not the "data the controller passes to the view."
fn extract_ivar_assignments(expr: &Expr, out: &mut HashMap<Symbol, Ty>) {
    match &*expr.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            if let Some(ty) = value.ty.clone() {
                out.insert(name.clone(), ty);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                extract_ivar_assignments(e, out);
            }
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            extract_ivar_assignments(then_branch, out);
            extract_ivar_assignments(else_branch, out);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            extract_ivar_assignments(expr, out);
            extract_ivar_assignments(fallback, out);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms {
                extract_ivar_assignments(&arm.body, out);
            }
        }
        _ => {}
    }
}

// Diagnostic emission -----------------------------------------------------

/// A single unresolved-type finding from the analyzer's output. Produced by
/// walking the annotated IR after `Analyzer::analyze` populated types;
/// anything that matters for typed emission but ended up as `Ty::Var(0)`
/// (unknown) generates one of these.
///
/// We accumulate diagnostics rather than aborting: a Rails program with
/// one unresolved site should still transpile the rest, and the set of
/// diagnostics is the "work list" for filling registry gaps or adding
/// annotations. Zero diagnostics = the program is in the analyzable subset.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DiagnosticKind {
    /// `@name` read at a site where no action seeded the ivar — the
    /// controller→view channel (or before_action flow) didn't bind it.
    IvarUnresolved { name: Symbol },
    /// `recv.method(...)` where `recv` has a known type but the method
    /// isn't in the registry for that type. Indicates a dialect gap —
    /// add the method to the relevant class/table lookup.
    SendDispatchFailed { method: Symbol, recv_ty: Ty },
}

/// Walk an analyzed `App` collecting every position where typing failed
/// in a way that matters for downstream typed emission. Does not modify
/// the IR — purely a read pass.
///
/// Scope of what's reported:
/// - Ivar reads whose `ty` remained `Ty::Var(0)`.
/// - Send calls with a concrete receiver type whose method wasn't found.
///
/// Deliberately NOT reported (noise suppression):
/// - Bare-name Sends whose receiver is implicit-self / None. Views without
///   a self_ty call many helpers we don't model (e.g. `csrf_meta_tags`);
///   flagging each would drown real diagnostics. Once helpers land via
///   the dialect registry expansion, this filter can be relaxed.
/// - Sends whose receiver itself is unknown. The root cause is upstream;
///   reporting both duplicates signal.
pub fn diagnose(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            diagnose_expr(&action.body, &mut out);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            diagnose_expr(&scope.body, &mut out);
        }
        for method in model.methods() {
            diagnose_expr(&method.body, &mut out);
        }
    }
    for view in &app.views {
        diagnose_expr(&view.body, &mut out);
    }
    out
}

/// A type is "unknown" if it's `None` or `Ty::Var(n)` (a placeholder the
/// analyzer set for positions it couldn't resolve).
fn is_unknown_ty(ty: Option<&Ty>) -> bool {
    match ty {
        None => true,
        Some(Ty::Var { .. }) => true,
        _ => false,
    }
}

fn diagnose_expr(expr: &Expr, out: &mut Vec<Diagnostic>) {
    match &*expr.node {
        ExprNode::Ivar { name } => {
            if is_unknown_ty(expr.ty.as_ref()) {
                out.push(Diagnostic {
                    span: expr.span,
                    kind: DiagnosticKind::IvarUnresolved { name: name.clone() },
                    message: format!("@{} has no known type", name.as_str()),
                });
            }
        }
        ExprNode::Send { recv: Some(r), method, .. } => {
            if !is_unknown_ty(r.ty.as_ref()) && is_unknown_ty(expr.ty.as_ref()) {
                let recv_ty = r.ty.clone().unwrap_or_else(unknown);
                out.push(Diagnostic {
                    span: expr.span,
                    kind: DiagnosticKind::SendDispatchFailed {
                        method: method.clone(),
                        recv_ty: recv_ty.clone(),
                    },
                    message: format!(
                        "no known method `{}` on {:?}",
                        method.as_str(),
                        recv_ty,
                    ),
                });
            }
        }
        _ => {}
    }

    // Recurse into children so we surface every unresolved position.
    match &*expr.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                diagnose_expr(r, out);
            }
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                diagnose_expr(k, out);
                diagnose_expr(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    diagnose_expr(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            diagnose_expr(left, out);
            diagnose_expr(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            diagnose_expr(cond, out);
            diagnose_expr(then_branch, out);
            diagnose_expr(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            diagnose_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    diagnose_expr(g, out);
                }
                diagnose_expr(&arm.body, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            diagnose_expr(value, out);
            diagnose_expr(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            diagnose_expr(body, out);
        }
        ExprNode::Apply { fun, args, block } => {
            diagnose_expr(fun, out);
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Assign { target, value } => {
            diagnose_expr(value, out);
            if let LValue::Attr { recv, .. } = target {
                diagnose_expr(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                diagnose_expr(recv, out);
                diagnose_expr(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                diagnose_expr(a, out);
            }
        }
        ExprNode::Raise { value } => diagnose_expr(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. } => {}
    }
}

fn union_of(a: Ty, b: Ty) -> Ty {
    if a == b {
        a
    } else {
        Ty::Union { variants: vec![a, b] }
    }
}

fn union_many(mut tys: Vec<Ty>) -> Ty {
    match tys.len() {
        0 => Ty::Nil,
        1 => tys.pop().unwrap(),
        _ => Ty::Union { variants: tys },
    }
}
