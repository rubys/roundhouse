//! Effect collection: walk a typed expression tree and attach per-node
//! `EffectSet`s (DbRead/DbWrite via the adapter's AR classification, Io for
//! controller render/redirect). Inherent `Analyzer` methods extracted
//! verbatim from `src/analyze/mod.rs` (pure code motion).

use std::collections::{BTreeSet, HashMap};

use crate::adapter::ArMethodKind;
use crate::App;
use crate::dialect::ControllerBodyItem;
use crate::effect::{Effect, EffectSet};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::Ctx;

impl super::Analyzer {
    /// Walk every live body once after the typing fixpoint. Effect
    /// annotation does not feed harvest/unify/`inference_sig`, so doing
    /// it inside each typing round (Campfire: 1 initial + 8 retypes,
    /// and controllers collect on Phase A plus each Phase B sweep)
    /// only repeated the same tree walk. `visit_effects` reads `self_ty`
    /// for implicit-self Sends and the already-typed receiver `ty`; it
    /// does not read ivar/local bindings.
    pub(super) fn stamp_body_effects(&self, app: &mut App) {
        let _stamp = crate::timings::begin("stamp effects");
        for controller in &mut app.controllers {
            let id = controller.name.clone();
            for item in &mut controller.body {
                match item {
                    ControllerBodyItem::Action { action, .. } => {
                        action.effects = self.stamp_expr(&id, &mut action.body);
                    }
                    ControllerBodyItem::ClassMethod { method, .. } => {
                        self.stamp_method(&id, method);
                    }
                    _ => {}
                }
            }
        }
        for model in &mut app.models {
            let id = model.name.clone();
            for method in model.methods_mut() {
                self.stamp_method(&id, method);
            }
        }
        for lc in &mut app.library_classes {
            let id = lc.name.clone();
            for method in &mut lc.methods {
                self.stamp_method(&id, method);
            }
        }
        for module in &mut app.test_modules {
            let id = module.name.clone();
            for method in &mut module.helpers {
                self.stamp_method(&id, method);
            }
        }
        if let Some(expr) = app.seeds.as_mut() {
            let _ = self.collect_effects(expr, &Ctx::default());
        }

        let mut by_key: HashMap<(ClassId, Symbol), EffectSet> = HashMap::new();
        for controller in &app.controllers {
            for action in controller.actions() {
                by_key.insert(
                    (controller.name.clone(), action.name.clone()),
                    action.effects.clone(),
                );
            }
            for method in controller.class_methods() {
                by_key.insert(
                    (controller.name.clone(), method.name.clone()),
                    method.effects.clone(),
                );
            }
        }
        for lc in &app.library_classes {
            for method in &lc.methods {
                by_key.insert((lc.name.clone(), method.name.clone()), method.effects.clone());
            }
        }
        let parents: HashMap<ClassId, Option<ClassId>> = app
            .controllers
            .iter()
            .map(|c| (c.name.clone(), c.parent.clone()))
            .collect();
        for resolution in app.controller_resolutions.values_mut() {
            for rf in &mut resolution.filter_chain {
                if rf.filter.kind.is_skip() {
                    continue;
                }
                rf.effects = lookup_filter_effects(&by_key, &parents, rf);
            }
        }
    }

    fn stamp_expr(&self, class: &ClassId, expr: &mut Expr) -> EffectSet {
        let ctx = Ctx {
            self_ty: Some(Ty::Class {
                id: class.clone(),
                args: vec![].into(),
            }),
            ..Ctx::default()
        };
        self.collect_effects(expr, &ctx)
    }

    fn stamp_method(&self, class: &ClassId, method: &mut crate::dialect::MethodDef) {
        method.effects = self.stamp_expr(class, &mut method.body);
        if let Some(Ty::Fn { effects, .. }) = &mut method.signature {
            *effects = method.effects.clone();
        }
    }

    pub(super) fn collect_effects(&self, expr: &mut Expr, ctx: &Ctx) -> EffectSet {
        let mut set = BTreeSet::new();
        self.visit_effects(expr, ctx, &mut set);
        EffectSet { effects: set }
    }

    /// Walk a typed expression tree computing each node's *local* effects
    /// (those the node itself contributes — typically only non-empty for
    /// `Send` onto an effectful method) and writing them to `expr.effects`.
    /// The running aggregate `out` collects effects across the subtree so
    /// the caller can still populate per-action / per-method totals.
    ///
    /// Two-pass analyze (before_action seeding) calls this a second time
    /// with a richer ctx; every per-node `expr.effects` write here
    /// overwrites the earlier value, so annotations stay consistent with
    /// the final typed tree.
    fn visit_effects(&self, expr: &mut Expr, ctx: &Ctx, out: &mut BTreeSet<Effect>) {
        let mut local: BTreeSet<Effect> = BTreeSet::new();

        match &mut *expr.node {
            ExprNode::Lit { .. }
            | ExprNode::Var { .. }
            | ExprNode::Ivar { .. }
            | ExprNode::Const { .. }
            | ExprNode::Retry
            | ExprNode::Redo
            | ExprNode::ForwardArgs
            | ExprNode::ForwardKeywords
            | ExprNode::Defined { .. }
            | ExprNode::SelfRef => {}

            ExprNode::ForwardKeywordsWithPairs { entries } => {
                for (key, value) in entries {
                    self.visit_effects(key, ctx, out);
                    self.visit_effects(value, ctx, out);
                }
            }

            ExprNode::Return { value } => self.visit_effects(value, ctx, out),

            ExprNode::Super { args } => {
                if let Some(args) = args {
                    for a in args {
                        self.visit_effects(a, ctx, out);
                    }
                }
            }

            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                self.visit_effects(body, ctx, out);
                for rc in rescues {
                    for c in &mut rc.classes {
                        self.visit_effects(c, ctx, out);
                    }
                    self.visit_effects(&mut rc.body, ctx, out);
                }
                if let Some(e) = else_branch {
                    self.visit_effects(e, ctx, out);
                }
                if let Some(e) = ensure {
                    self.visit_effects(e, ctx, out);
                }
            }

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
            ExprNode::MethodRef { recv, .. } => {
                // `method(:name)` / `recv.method(:name)` — binding a
                // Method object is pure (mirrors Lambda: only invoking
                // it has effects, and there is no first-class Fn-effect
                // tracking yet). Evaluating an explicit receiver can
                // itself be effectful, so recurse into it.
                if let Some(r) = recv {
                    self.visit_effects(r, ctx, out);
                }
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
                // Local effects for THIS Send — the dispatched method's
                // declared side-effect class, determined from the receiver
                // type + method name. Sub-expressions (receiver, args,
                // block) contribute their own local effects via their own
                // annotations; not folded into this node's `local`.
                if let Some(ty) = recv_ty {
                    self.contribute_send_effect(&ty, method, &mut local);
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
                    if let Some(g) = &mut arm.guard { self.visit_effects(g, ctx, out); }
                    self.visit_effects(&mut arm.body, ctx, out);
                }
            }
            ExprNode::CaseMatch { scrutinee, arms, else_body } => {
                self.visit_effects(scrutinee, ctx, out);
                for arm in arms {
                    self.visit_match_pattern_effects(&mut arm.pattern, ctx, out);
                    if let Some((_, g)) = &mut arm.guard { self.visit_effects(g, ctx, out); }
                    self.visit_effects(&mut arm.body, ctx, out);
                }
                if let Some(e) = else_body { self.visit_effects(e, ctx, out); }
            }
            ExprNode::MatchPredicate { value, pattern }
            | ExprNode::MatchRequired { value, pattern } => {
                self.visit_effects(value, ctx, out);
                self.visit_match_pattern_effects(pattern, ctx, out);
            }
            ExprNode::Seq { exprs } => {
                for e in exprs { self.visit_effects(e, ctx, out); }
            }
            ExprNode::Assign { target, value }
            | ExprNode::OpAssign { target, value, .. } => {
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
            ExprNode::Next { value } | ExprNode::Break { value } => {
                if let Some(v) = value { self.visit_effects(v, ctx, out); }
            }
            ExprNode::Splat { value } | ExprNode::KeywordSplat { value } => self.visit_effects(value, ctx, out),
            ExprNode::MultiAssign { targets, value } => {
                self.visit_effects(value, ctx, out);
                for target in targets.iter_mut() {
                    if let LValue::Attr { recv, .. } = target {
                        self.visit_effects(recv, ctx, out);
                    }
                    if let LValue::Index { recv, index } = target {
                        self.visit_effects(recv, ctx, out);
                        self.visit_effects(index, ctx, out);
                    }
                }
            }
            ExprNode::While { cond, body, .. } => {
                self.visit_effects(cond, ctx, out);
                self.visit_effects(body, ctx, out);
            }
            ExprNode::Range { begin, end, .. } => {
                if let Some(b) = begin { self.visit_effects(b, ctx, out); }
                if let Some(e) = end { self.visit_effects(e, ctx, out); }
            }
            ExprNode::Cast { value, .. } => self.visit_effects(value, ctx, out),
        }

        // Persist local effects onto this node and feed the running
        // aggregate. Overwrite rather than merge: the caller may re-invoke
        // (two-pass before_action seeding), and each pass computes local
        // effects from scratch against the current typed tree.
        out.extend(local.iter().cloned());
        expr.effects = EffectSet { effects: local };
    }

    /// Visit every `Expr` embedded in a `MatchPattern` — a `Value`'s
    /// test expression, or an `Array`/`Find`/`Hash` pattern's narrowing
    /// `constant` — the effects-walk mirror of the body-typer's
    /// `analyze_match_pattern_constants`. A pattern's own shape (which
    /// keys/elements it destructures) never contributes an effect
    /// itself; only the embedded expressions can.
    fn visit_match_pattern_effects(
        &self,
        pattern: &mut crate::expr::MatchPattern,
        ctx: &Ctx,
        out: &mut BTreeSet<Effect>,
    ) {
        pattern.for_each_expr_mut(&mut |expr| self.visit_effects(expr, ctx, out));
    }

    fn contribute_send_effect(&self, recv_ty: &Ty, method: &Symbol, out: &mut BTreeSet<Effect>) {
        let Ty::Class { id, .. } = recv_ty else { return };
        let Some(cls) = self.classes.get(id) else { return };

        // AR methods on model classes: DbRead / DbWrite against the
        // bound table. The adapter owns the classification — swapping
        // adapters changes which methods produce effects (e.g., an
        // IndexedDB adapter can return Unknown for methods it can't
        // implement, making them silent at the effect level and
        // diagnostic-bearing downstream).
        //
        // Terminal-vs-builder gating: Relation-builder methods
        // (`where`, `limit`, `order`, `includes`, `joins`, `group`,
        // `having`, `preload`, `distinct`) return a lazy Relation
        // that hasn't executed SQL. Under an async backend, awaiting
        // each builder link would emit one round-trip per chain
        // step instead of the single round-trip the terminal call
        // actually triggers. Skipping the effect attachment here
        // means those builder Sends carry no effect in the IR — the
        // await machinery walks past them to the terminal step that
        // does. ChainKind::Terminal / NotApplicable / missing entry
        // all keep the effect; only explicit Builder skips.
        if let Some(table) = &cls.table {
            let kind = self.adapter.classify_ar_method(method.as_str());
            let is_builder_read =
                matches!(kind, ArMethodKind::Read) && self.is_builder_chain(method.as_str());
            if !is_builder_read {
                match kind {
                    ArMethodKind::Read => {
                        out.insert(Effect::DbRead { table: table.clone() });
                    }
                    ArMethodKind::Write => {
                        out.insert(Effect::DbWrite { table: table.clone() });
                    }
                    ArMethodKind::Unknown => {}
                }
            }
        }

        // Controller-side IO effects — Rails dialect, not adapter
        // territory. Every backend renders views and redirects the
        // same way at the effect level; the concrete implementation
        // lives in each target's runtime, not here. The receiver is the
        // controller's own class now (self_ty), so match any controller
        // by the Rails `*Controller` convention — `ApplicationController`,
        // `StoriesController`, etc. — not just the literal base. (View
        // renders dispatch with no receiver and never reach here.)
        if id.0.as_str().ends_with("Controller") {
            match method.as_str() {
                "render" | "redirect_to" | "redirect_back_or_to" | "head" => {
                    out.insert(Effect::Io);
                }
                _ => {}
            }
        }
    }
}

/// Filter-target effects follow Ruby method lookup: the class that
/// carried the filter into the chain, then its parent controllers
/// nearest-first, then `defined_in` (concern modules). A subclass
/// `before_action :load_room` whose method lives on the parent would
/// miss both `(included_via, target)` and `(defined_in, target)` —
/// those names are the subclass, where the method was never stamped.
fn lookup_filter_effects(
    by_key: &HashMap<(ClassId, Symbol), EffectSet>,
    parents: &HashMap<ClassId, Option<ClassId>>,
    rf: &crate::app::ResolvedFilter,
) -> EffectSet {
    let target = &rf.filter.target;
    if let Some(effects) = by_key.get(&(rf.included_via.clone(), target.clone())) {
        return effects.clone();
    }
    let mut current = rf.included_via.clone();
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current.clone()) {
        let Some(parent) = parents.get(&current).and_then(|p| p.clone()) else {
            break;
        };
        if let Some(effects) = by_key.get(&(parent.clone(), target.clone())) {
            return effects.clone();
        }
        current = parent;
    }
    by_key
        .get(&(rf.defined_in.clone(), target.clone()))
        .cloned()
        .unwrap_or_default()
}
