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
use crate::effect::{Effect, EffectSet};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TableRef, TyVar};
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
            cls.class_methods.insert(Symbol::from("count"), Ty::Int);
            cls.class_methods.insert(Symbol::from("new"), self_ty.clone());
            cls.class_methods.insert(Symbol::from("create"), self_ty.clone());

            // Instance methods from schema-derived attributes.
            for (name, ty) in &model.attributes.fields {
                cls.instance_methods.insert(name.clone(), ty.clone());
            }
            // Associations as instance methods (return types derived from cardinality).
            for assoc in &model.associations {
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
            };
            for action in &mut controller.actions {
                self.analyze_expr(&mut action.body, &ctx);
                action.effects = self.collect_effects(&action.body, &ctx);
            }
        }
        for model in &mut app.models {
            let class_ctx = Ctx {
                self_ty: Some(Ty::Class { id: model.name.clone(), args: vec![] }),
                ivar_bindings: HashMap::new(),
            };
            for scope in &mut model.scopes {
                self.analyze_expr(&mut scope.body, &class_ctx);
            }
            for method in &mut model.methods {
                self.analyze_expr(&mut method.body, &class_ctx);
                method.effects = self.collect_effects(&method.body, &class_ctx);
            }
        }
        for view in &mut app.views {
            self.analyze_expr(&mut view.body, &Ctx::default());
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

            ExprNode::Var { .. } => unknown(), // local scope tracking is future work

            ExprNode::Ivar { name } => {
                ctx.ivar_bindings.get(name).cloned().unwrap_or_else(unknown)
            }

            ExprNode::Let { value, body, .. } => {
                self.analyze_expr(value, ctx);
                self.analyze_expr(body, ctx)
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
                let recv_ty = match recv.as_mut() {
                    Some(r) => Some(self.analyze_expr(r, ctx)),
                    None => ctx.self_ty.clone(),
                };
                for a in args.iter_mut() { self.analyze_expr(a, ctx); }
                if let Some(b) = block { self.analyze_expr(b, ctx); }
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
                // Within a Seq, walk statements in order and thread ivar
                // bindings: `@post = Post.find(...)` in stmt i lets stmt
                // i+1 resolve `@post` through `ctx.ivar_bindings`.
                let mut local_ctx = ctx.clone();
                let mut last = Ty::Nil;
                for e in exprs.iter_mut() {
                    last = self.analyze_expr(e, &local_ctx);
                    if let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*e.node {
                        if let Some(ty) = e.ty.clone() {
                            local_ctx.ivar_bindings.insert(name.clone(), ty);
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
    match method.as_str() {
        "length" | "size" | "count" => Ty::Int,
        "first" | "last" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "[]" => Ty::Union {
            variants: vec![elem.clone(), Ty::Nil],
        },
        "each" | "map" | "select" | "reject" | "sort" | "reverse" => {
            Ty::Array { elem: Box::new(elem.clone()) }
        }
        _ => unknown(),
    }
}

fn hash_method(method: &Symbol, value: &Ty) -> Ty {
    match method.as_str() {
        "[]" => Ty::Union { variants: vec![value.clone(), Ty::Nil] },
        "length" | "size" | "count" => Ty::Int,
        "values" => Ty::Array { elem: Box::new(value.clone()) },
        "empty?" => Ty::Bool,
        _ => unknown(),
    }
}

fn str_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "length" | "size" | "bytesize" => Ty::Int,
        "upcase" | "downcase" | "strip" | "chomp" | "chop" | "reverse" | "to_s" => Ty::Str,
        "to_i" => Ty::Int,
        "to_f" => Ty::Float,
        "empty?" | "blank?" | "present?" => Ty::Bool,
        _ => unknown(),
    }
}

fn int_method(method: &Symbol) -> Ty {
    match method.as_str() {
        "to_s" => Ty::Str,
        "to_i" | "abs" | "succ" | "pred" => Ty::Int,
        "to_f" => Ty::Float,
        "zero?" | "positive?" | "negative?" | "even?" | "odd?" => Ty::Bool,
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
