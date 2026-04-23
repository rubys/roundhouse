//! Body-typer: the Rails-agnostic core of type inference.
//!
//! Given an [`Expr`] + a [`Ctx`] + a method dispatch table (a
//! `HashMap<ClassId, ClassInfo>`), walks the tree and annotates each
//! node's `ty` field. This module knows nothing about schemas,
//! controllers, `before_action` chains, or any other Rails dialect —
//! it's pure "Ruby body type inference against known signatures."
//!
//! The Rails-aware [`super::Analyzer`] pre-computes the dispatch
//! table from `App.models` and then threads a [`BodyTyper`] over each
//! method body, action body, and view body.
//!
//! Runtime-extraction code (src/runtime_src.rs) uses the same
//! [`BodyTyper`] with a simpler dispatch table (no user classes, just
//! the primitive method tables).

use std::collections::HashMap;

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{Row, Ty};

/// Recursion context — what `self` is, what locals/ivars are in scope.
/// Immutable during descent; clone to enter a new scope (Let body,
/// block body, Seq walk with new ivar/local bindings).
#[derive(Clone, Default)]
pub struct Ctx {
    pub self_ty: Option<Ty>,
    /// Ivar bindings observed as a `Seq` walks its statements in order.
    /// `@post = Post.find(...)` in stmt 1 lets `@post.destroy` in stmt 2
    /// dispatch correctly.
    pub ivar_bindings: HashMap<Symbol, Ty>,
    /// Local-variable bindings in the current scope: let-bound names,
    /// assignments accumulated through a `Seq`, and block parameters
    /// seeded from a receiver-aware dispatch.
    pub local_bindings: HashMap<Symbol, Ty>,
}

/// User-class dispatch data: table name (if any), instance shape,
/// class/instance method tables. Built by [`super::Analyzer`] from
/// Rails schema + conventions; the body-typer reads it.
#[derive(Default)]
pub struct ClassInfo {
    /// If this class maps to a database table, which one.
    pub table: Option<crate::ident::TableRef>,
    /// Instance-state shape (columns + attr_accessor).
    pub attributes: Row,
    /// Methods callable on the class itself: `Post.all`, `Post.find(id)`.
    pub class_methods: HashMap<Symbol, Ty>,
    /// Methods callable on an instance: `post.title`, `post.destroy`.
    pub instance_methods: HashMap<Symbol, Ty>,
}

/// Reusable body-type walker. Holds a borrow of the dispatch table so
/// repeated `analyze_expr` calls reuse the same lookup structures
/// without cloning.
pub struct BodyTyper<'a> {
    classes: &'a HashMap<ClassId, ClassInfo>,
}

impl<'a> BodyTyper<'a> {
    pub fn new(classes: &'a HashMap<ClassId, ClassInfo>) -> Self {
        Self { classes }
    }

    /// Analyze an expression: compute its type, populate `expr.ty`,
    /// return the computed type. Recurses into sub-expressions, which
    /// in turn get their `ty` populated.
    pub fn analyze_expr(&self, expr: &mut Expr, ctx: &Ctx) -> Ty {
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
                let pred = extract_narrowing(cond);
                let t = match &pred {
                    Some(p) => {
                        let then_ctx = apply_narrowing(ctx, p, true);
                        self.analyze_expr(then_branch, &then_ctx)
                    }
                    None => self.analyze_expr(then_branch, ctx),
                };
                let e = match &pred {
                    Some(p) => {
                        let else_ctx = apply_narrowing(ctx, p, false);
                        self.analyze_expr(else_branch, &else_ctx)
                    }
                    None => self.analyze_expr(else_branch, ctx),
                };
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

// Narrowing -----------------------------------------------------------

/// A variable reference that narrowing can target: either a local
/// binding (`x`) or an instance variable (`@x`).
enum VarKey {
    Local(Symbol),
    Ivar(Symbol),
}

/// A condition that narrows a variable's type in the branches of an
/// `if`. Only nil-shaped and class-shaped predicates are recognized —
/// more complex conditions fall through with no narrowing applied.
enum NarrowPred {
    /// `x.nil?` or `x == nil` — true in then, false in else.
    IsNil(VarKey),
    /// `!x.nil?` or `x != nil` — false in then, true in else.
    IsNotNil(VarKey),
    /// `x.is_a?(T)` — narrow to T in then, remove T from union in else.
    IsA(VarKey, Ty),
    /// `!x.is_a?(T)` — inverse.
    IsNotA(VarKey, Ty),
}

fn extract_narrowing(cond: &Expr) -> Option<NarrowPred> {
    match &*cond.node {
        // Ruby's `!` is a method call: `!x` parses as `x.!`. So
        // `!x.nil?` is Send(method="!", recv=Some(Send(method="nil?", recv=Var(x)))).
        ExprNode::Send { recv: Some(inner), method, args, .. }
            if method.as_str() == "!" && args.is_empty() =>
        {
            extract_narrowing(inner).map(negate_pred)
        }
        ExprNode::Send { recv: Some(target), method, args, .. } => {
            match (method.as_str(), args.as_slice()) {
                ("nil?", []) => var_key(target).map(NarrowPred::IsNil),
                ("==", [arg]) if is_nil_lit(arg) => {
                    var_key(target).map(NarrowPred::IsNil)
                }
                ("!=", [arg]) if is_nil_lit(arg) => {
                    var_key(target).map(NarrowPred::IsNotNil)
                }
                ("is_a?" | "kind_of?" | "instance_of?", [arg]) => {
                    let key = var_key(target)?;
                    let ty = const_to_ty(arg)?;
                    Some(NarrowPred::IsA(key, ty))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn negate_pred(p: NarrowPred) -> NarrowPred {
    match p {
        NarrowPred::IsNil(k) => NarrowPred::IsNotNil(k),
        NarrowPred::IsNotNil(k) => NarrowPred::IsNil(k),
        NarrowPred::IsA(k, t) => NarrowPred::IsNotA(k, t),
        NarrowPred::IsNotA(k, t) => NarrowPred::IsA(k, t),
    }
}

fn var_key(e: &Expr) -> Option<VarKey> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(VarKey::Local(name.clone())),
        ExprNode::Ivar { name } => Some(VarKey::Ivar(name.clone())),
        _ => None,
    }
}

fn is_nil_lit(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// A constant path used as a class argument to `is_a?` — map built-in
/// class names to their structural types, user classes to `Ty::Class`.
fn const_to_ty(e: &Expr) -> Option<Ty> {
    let ExprNode::Const { path } = &*e.node else {
        return None;
    };
    let name = path.last()?;
    Some(match name.as_str() {
        "Integer" | "Numeric" => Ty::Int,
        "Float" => Ty::Float,
        "String" => Ty::Str,
        "Symbol" => Ty::Sym,
        "NilClass" => Ty::Nil,
        "TrueClass" | "FalseClass" => Ty::Bool,
        other => Ty::Class {
            id: ClassId(Symbol::from(other)),
            args: vec![],
        },
    })
}

fn apply_narrowing(ctx: &Ctx, pred: &NarrowPred, then_branch: bool) -> Ctx {
    let mut new_ctx = ctx.clone();
    match pred {
        NarrowPred::IsNil(k) | NarrowPred::IsNotNil(k) => {
            let is_is_nil = matches!(pred, NarrowPred::IsNil(_));
            let narrow_to_nil = is_is_nil == then_branch;
            narrow_binding(&mut new_ctx, k, |current| {
                if narrow_to_nil {
                    Ty::Nil
                } else {
                    remove_nil(current)
                }
            });
        }
        NarrowPred::IsA(k, ty) | NarrowPred::IsNotA(k, ty) => {
            let is_is_a = matches!(pred, NarrowPred::IsA(_, _));
            let narrow_to_ty = is_is_a == then_branch;
            narrow_binding(&mut new_ctx, k, |current| {
                if narrow_to_ty {
                    intersect_with(current, ty)
                } else {
                    remove_variant(current, ty)
                }
            });
        }
    }
    new_ctx
}

fn narrow_binding<F: FnOnce(&Ty) -> Ty>(ctx: &mut Ctx, key: &VarKey, f: F) {
    let (name, bindings) = match key {
        VarKey::Local(n) => (n, &mut ctx.local_bindings),
        VarKey::Ivar(n) => (n, &mut ctx.ivar_bindings),
    };
    if let Some(current) = bindings.get(name).cloned() {
        let narrowed = f(&current);
        bindings.insert(name.clone(), narrowed);
    }
}

fn remove_nil(ty: &Ty) -> Ty {
    match ty {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| !matches!(v, Ty::Nil))
                .cloned()
                .collect();
            match kept.len() {
                0 => Ty::Nil,
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        // Not a union — if the type is bare Nil, the "non-nil" branch
        // is unreachable in Ruby; we keep Nil here (the analyzer doesn't
        // flag contradictions). For non-Nil concrete types, no change.
        other => other.clone(),
    }
}

/// Given a current type and a narrower one, return the narrower form.
/// `String | Nil ∩ String = String`; `Post ∩ Post = Post`; anything
/// else returns the narrower type on the assumption the check would
/// have succeeded (matches Ruby's `is_a?` semantics at run time).
fn intersect_with(current: &Ty, narrower: &Ty) -> Ty {
    match current {
        Ty::Union { variants } => {
            // Keep only variants compatible with the narrower type.
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| ty_compatible(v, narrower))
                .cloned()
                .collect();
            match kept.len() {
                0 => narrower.clone(),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        _ => narrower.clone(),
    }
}

/// Remove variants matching `ty` from a union (for `is_a?` else-branch).
fn remove_variant(current: &Ty, ty: &Ty) -> Ty {
    match current {
        Ty::Union { variants } => {
            let kept: Vec<Ty> = variants
                .iter()
                .filter(|v| !ty_compatible(v, ty))
                .cloned()
                .collect();
            match kept.len() {
                0 => current.clone(),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union { variants: kept },
            }
        }
        _ => current.clone(),
    }
}

/// Structural equality on types — pre-subtyping approximation.
/// Used only by narrowing today; full subtype checks can replace it
/// when polymorphism lands.
fn ty_compatible(a: &Ty, b: &Ty) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::ExprNode;
    use crate::ident::VarId;
    use crate::span::Span;

    fn synth(node: ExprNode) -> Expr {
        Expr::new(Span::synthetic(), node)
    }

    fn var(name: &str) -> Expr {
        synth(ExprNode::Var {
            id: VarId(0),
            name: Symbol::from(name),
        })
    }

    fn nil_lit() -> Expr {
        synth(ExprNode::Lit { value: Literal::Nil })
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        synth(ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        })
    }

    fn empty_classes() -> HashMap<ClassId, ClassInfo> {
        HashMap::new()
    }

    fn ctx_with_local(name: &str, ty: Ty) -> Ctx {
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(Symbol::from(name), ty);
        ctx
    }

    fn optional_str() -> Ty {
        Ty::Union {
            variants: vec![Ty::Str, Ty::Nil],
        }
    }

    #[test]
    fn if_nil_narrows_variable_to_nil_in_then_branch() {
        let cond = send(Some(var("x")), "nil?", vec![]);
        let pred = extract_narrowing(&cond).expect("narrowing detected");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn if_nil_narrows_variable_removing_nil_in_else_branch() {
        let cond = send(Some(var("x")), "nil?", vec![]);
        let pred = extract_narrowing(&cond).unwrap();
        let ctx = ctx_with_local("x", optional_str());
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(else_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
    }

    #[test]
    fn not_nil_is_inverse() {
        let inner = send(Some(var("x")), "nil?", vec![]);
        let cond = send(Some(inner), "!", vec![]);
        let pred = extract_narrowing(&cond).expect("negation recognized");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(else_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn explicit_equality_to_nil_narrows() {
        let cond = send(Some(var("x")), "==", vec![nil_lit()]);
        let pred = extract_narrowing(&cond).expect("== nil recognized");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn explicit_inequality_to_nil_narrows_inversely() {
        let cond = send(Some(var("x")), "!=", vec![nil_lit()]);
        let pred = extract_narrowing(&cond).expect("!= nil recognized");
        let ctx = ctx_with_local("x", optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(else_ctx.local_bindings[&Symbol::from("x")], Ty::Nil);
    }

    #[test]
    fn ivar_narrowing() {
        let ivar = synth(ExprNode::Ivar {
            name: Symbol::from("post"),
        });
        let cond = send(Some(ivar), "nil?", vec![]);
        let pred = extract_narrowing(&cond).unwrap();
        let mut ctx = Ctx::default();
        ctx.ivar_bindings
            .insert(Symbol::from("post"), optional_str());
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.ivar_bindings[&Symbol::from("post")], Ty::Nil);
        assert_eq!(else_ctx.ivar_bindings[&Symbol::from("post")], Ty::Str);
    }

    #[test]
    fn missing_binding_is_a_noop() {
        let cond = send(Some(var("x")), "nil?", vec![]);
        let pred = extract_narrowing(&cond).unwrap();
        let ctx = Ctx::default();
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert!(then_ctx.local_bindings.is_empty());
    }

    #[test]
    fn non_narrowing_condition_returns_none() {
        let cond = send(Some(var("x")), "length", vec![]);
        assert!(extract_narrowing(&cond).is_none());
    }

    #[test]
    fn is_a_string_narrows_to_str() {
        let class_ref = synth(ExprNode::Const {
            path: vec![Symbol::from("String")],
        });
        let cond = send(Some(var("x")), "is_a?", vec![class_ref]);
        let pred = extract_narrowing(&cond).expect("is_a? recognized");
        let mixed = Ty::Union {
            variants: vec![Ty::Str, Ty::Int, Ty::Nil],
        };
        let ctx = ctx_with_local("x", mixed);
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        let else_ctx = apply_narrowing(&ctx, &pred, false);
        assert_eq!(then_ctx.local_bindings[&Symbol::from("x")], Ty::Str);
        assert_eq!(
            else_ctx.local_bindings[&Symbol::from("x")],
            Ty::Union {
                variants: vec![Ty::Int, Ty::Nil],
            }
        );
    }

    #[test]
    fn is_a_user_class_narrows_to_class_ty() {
        let class_ref = synth(ExprNode::Const {
            path: vec![Symbol::from("Post")],
        });
        let cond = send(Some(var("x")), "is_a?", vec![class_ref]);
        let pred = extract_narrowing(&cond).unwrap();
        let ctx = ctx_with_local(
            "x",
            Ty::Class {
                id: ClassId(Symbol::from("Post")),
                args: vec![],
            },
        );
        let then_ctx = apply_narrowing(&ctx, &pred, true);
        assert_eq!(
            then_ctx.local_bindings[&Symbol::from("x")],
            Ty::Class {
                id: ClassId(Symbol::from("Post")),
                args: vec![]
            }
        );
    }

    #[test]
    fn end_to_end_if_nil_narrows_through_analyzer() {
        // Build: `if x.nil?; x; else; x.length; end` — with x: String | Nil,
        // the If's type should be Nil | Int (then is Nil, else is Int
        // because x narrows to String and String#length → Int).
        let then_branch = var("x");
        let else_branch = send(Some(var("x")), "length", vec![]);
        let if_expr = synth(ExprNode::If {
            cond: send(Some(var("x")), "nil?", vec![]),
            then_branch,
            else_branch,
        });
        let mut expr = if_expr;

        let classes = empty_classes();
        let typer = BodyTyper::new(&classes);
        let ctx = ctx_with_local("x", optional_str());
        let t = typer.analyze_expr(&mut expr, &ctx);

        // Result should be the union of (Nil) | (Int) — i.e., { Nil, Int }.
        let variants = match t {
            Ty::Union { variants } => variants,
            other => panic!("expected Union, got {other:?}"),
        };
        assert!(variants.contains(&Ty::Nil), "variants: {variants:?}");
        assert!(variants.contains(&Ty::Int), "variants: {variants:?}");
    }
}
