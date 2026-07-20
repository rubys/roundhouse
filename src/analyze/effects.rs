//! Effect collection: walk a typed expression tree and attach per-node
//! `EffectSet`s (DbRead/DbWrite via the adapter's AR classification, Io for
//! controller render/redirect). Inherent `Analyzer` methods extracted
//! verbatim from `src/analyze/mod.rs` (pure code motion).

use std::collections::BTreeSet;

use crate::adapter::ArMethodKind;
use crate::effect::{Effect, EffectSet};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::ty::Ty;

use super::Ctx;

impl super::Analyzer {
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
            | ExprNode::SelfRef => {}

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
            ExprNode::Splat { value } => self.visit_effects(value, ctx, out),
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
                "render" | "redirect_to" | "head" => {
                    out.insert(Effect::Io);
                }
                _ => {}
            }
        }
    }
}
