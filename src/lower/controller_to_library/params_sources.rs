//! Params provenance at individual local reads. Names alone are insufficient:
//! a later assignment, a conditional write, or a block parameter can change
//! what a local contains. This table does not change the analyzer's types.

use std::collections::{BTreeMap, HashMap};

use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

use super::params::{params_source_class, ParamsSpecs};

#[derive(Clone, PartialEq, Eq)]
enum Source {
    Params(ClassId),
    Hash,
    Nested,
}

type Locals = BTreeMap<Symbol, Source>;

pub(super) struct ParamsSources {
    reads: HashMap<(Span, Symbol), Option<Source>>,
    helpers: BTreeMap<Symbol, ClassId>,
}

impl ParamsSources {
    pub(super) fn new(
        body: &Expr,
        helpers: BTreeMap<Symbol, ClassId>,
        specs: &ParamsSpecs,
    ) -> Self {
        let mut sources = Self { reads: HashMap::new(), helpers };
        sources.visit(body, &mut Locals::new(), specs);
        sources
    }

    pub(super) fn class(&self, expr: &Expr, specs: &ParamsSpecs) -> Option<ClassId> {
        match self.source(expr, specs) {
            Some(Source::Params(class)) => Some(class),
            _ => None,
        }
    }

    pub(super) fn nested(&self, expr: &Expr, specs: &ParamsSpecs) -> bool {
        self.source(expr, specs) == Some(Source::Nested)
    }

    fn source(&self, expr: &Expr, specs: &ParamsSpecs) -> Option<Source> {
        match &*expr.node {
            ExprNode::Var { name, .. } => {
                self.reads.get(&(expr.span, name.clone())).cloned().flatten()
            }
            ExprNode::Send { recv: Some(recv), method, block: None, .. }
                if matches!(method.as_str(), "except" | "compact") =>
                    self.source(recv, specs).filter(|source|
                        matches!(source, Source::Params(_) | Source::Hash)),
            ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
                if matches!(method.as_str(), "delete" | "[]") && args.len() == 1 => {
                    let class = self.class(recv, specs)?;
                    let spec = specs.by_class(&class)?;
                    if !spec.has_nested_value(&args[0]) { return None; }
                    Some(Source::Nested)
                }
            _ => params_source_class(expr, &self.helpers, specs).map(Source::Params),
        }
    }

    pub(super) fn is_hash(&self, expr: &Expr) -> bool {
        match &*expr.node {
            ExprNode::Hash { .. } => true,
            ExprNode::Var { name, .. } => {
                self.reads.get(&(expr.span, name.clone())) == Some(&Some(Source::Hash))
            }
            _ => matches!(expr.ty.as_ref(), Some(crate::ty::Ty::Hash { .. })),
        }
    }

    fn visit(&mut self, expr: &Expr, locals: &mut Locals, specs: &ParamsSpecs) -> Option<Source> {
        match &*expr.node {
            ExprNode::Var { name, .. } => {
                let class = locals.get(name).cloned();
                // Synthetic or duplicated reads are not proof of a source
                // binding. A conflicting visit must never upgrade a read.
                if !expr.span.is_synthetic() {
                    self.reads.entry((expr.span, name.clone()))
                        .and_modify(|old| { if *old != class { *old = None; } })
                        .or_insert(class.clone());
                }
                class
            }
            ExprNode::Hash { .. } => {
                expr.node.for_each_child(&mut |child| { self.visit(child, locals, specs); });
                Some(Source::Hash)
            }
            ExprNode::Seq { exprs } => {
                let mut class = None;
                for child in exprs { class = self.visit(child, locals, specs); }
                class
            }
            ExprNode::Assign { target, value } => {
                match target {
                    LValue::Attr { recv, .. } => { self.visit(recv, locals, specs); }
                    LValue::Index { recv, index } => {
                        self.visit(recv, locals, specs);
                        self.visit(index, locals, specs);
                    }
                    _ => {}
                }
                let class = self.visit(value, locals, specs);
                if let LValue::Var { name, .. } = target {
                    locals.remove(name);
                    if let Some(class) = &class { locals.insert(name.clone(), class.clone()); }
                }
                class
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                self.visit(cond, locals, specs);
                let mut other = locals.clone();
                let yes = self.visit(then_branch, locals, specs);
                let no = self.visit(else_branch, &mut other, specs);
                intersect(locals, &other);
                if yes == no { yes } else { None }
            }
            ExprNode::BoolOp { left, right, .. } => {
                self.visit(left, locals, specs);
                let skipped = locals.clone();
                self.visit(right, locals, specs);
                intersect(locals, &skipped);
                None
            }
            ExprNode::Lambda { params, rest_param, extra_params, block_param, body, .. } => {
                let mut inner = locals.clone();
                // A block can run repeatedly. Captured bindings written by
                // its body need not retain their entry class on a later run.
                forget_writes(body, &mut inner);
                for name in params.iter().chain(rest_param.iter()).chain(block_param.iter()) {
                    inner.remove(name);
                }
                for param in extra_params { inner.remove(&param.name); }
                for param in extra_params {
                    if let Some(default) = &param.default {
                        let skipped = inner.clone();
                        self.visit(default, &mut inner, specs);
                        intersect(&mut inner, &skipped);
                    }
                }
                self.visit(body, &mut inner, specs);
                // A block may write captured locals and may run zero times.
                // Do not assume its post-state at the enclosing call site.
                locals.clear();
                None
            }
            ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
                let result = self.visit(body, locals, specs);
                if let Some(other) = else_branch { self.visit(other, locals, specs); }
                for rescue in rescues {
                    // An exception can occur before any body assignment.
                    let mut rescued = Locals::new();
                    self.visit(&rescue.body, &mut rescued, specs);
                    intersect(locals, &rescued);
                }
                if let Some(ensure) = ensure { self.visit(ensure, locals, specs); }
                if rescues.is_empty() && else_branch.is_none() { result } else { None }
            }
            ExprNode::While { .. } | ExprNode::Case { .. } | ExprNode::CaseMatch { .. }
            | ExprNode::RescueModifier { .. } => {
                // No fixed-point or pattern-binding inference here. Each arm
                // may establish aliases within itself, but none escapes.
                locals.clear();
                expr.node.for_each_child(&mut |child| {
                    self.visit(child, &mut Locals::new(), specs);
                });
                None
            }
            ExprNode::OpAssign { .. } | ExprNode::MultiAssign { .. }
            | ExprNode::MatchPredicate { .. } | ExprNode::MatchRequired { .. } => {
                expr.node.for_each_child(&mut |child| { self.visit(child, locals, specs); });
                locals.clear();
                None
            }
            ExprNode::Send { recv, method, args, block, .. } => {
                let receiver = recv.as_ref().and_then(|r| self.visit(r, locals, specs));
                for arg in args { self.visit(arg, locals, specs); }
                if let Some(block) = block { self.visit(block, locals, specs); }
                if block.is_none() && matches!(method.as_str(), "except" | "compact") {
                    receiver.filter(|source| matches!(source, Source::Params(_) | Source::Hash))
                } else {
                    self.source(expr, specs)
                }
            }
            _ => {
                expr.node.for_each_child(&mut |child| { self.visit(child, locals, specs); });
                None
            }
        }
    }
}

fn intersect(locals: &mut Locals, other: &Locals) {
    locals.retain(|name, class| other.get(name) == Some(class));
}

fn forget_writes(expr: &Expr, locals: &mut Locals) {
    match &*expr.node {
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
            if let LValue::Var { name, .. } = target { locals.remove(name); }
        }
        ExprNode::MultiAssign { targets, .. } => {
            for target in targets {
                if let LValue::Var { name, .. } = target { locals.remove(name); }
            }
        }
        ExprNode::MatchPredicate { .. } | ExprNode::MatchRequired { .. } => { locals.clear(); }
        _ => {}
    }
    expr.node.for_each_child(&mut |child| forget_writes(child, locals));
}
