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

use std::collections::HashMap;

use crate::App;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, TyVar};
use crate::ty::{Row, Ty};

pub struct Analyzer {
    classes: HashMap<ClassId, ClassInfo>,
}

#[derive(Default)]
struct ClassInfo {
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

        Self { classes }
    }

    /// Walk the app, annotating every expression's `ty` field.
    pub fn analyze(&self, app: &mut App) {
        for controller in &mut app.controllers {
            for action in &mut controller.actions {
                self.analyze_expr(&mut action.body);
            }
        }
        for model in &mut app.models {
            for scope in &mut model.scopes {
                self.analyze_expr(&mut scope.body);
            }
            for method in &mut model.methods {
                self.analyze_expr(&mut method.body);
            }
        }
        for view in &mut app.views {
            self.analyze_expr(&mut view.body);
        }
    }

    fn analyze_expr(&self, expr: &mut Expr) -> Ty {
        let ty = self.compute(expr);
        expr.ty = Some(ty.clone());
        ty
    }

    fn compute(&self, expr: &mut Expr) -> Ty {
        match &mut *expr.node {
            ExprNode::Lit { value } => lit_ty(value),

            ExprNode::Const { path } => {
                // `Post` as an expression refers to the class.
                let name = path.last().cloned().unwrap_or_else(|| Symbol::from("?"));
                Ty::Class { id: ClassId(name), args: vec![] }
            }

            ExprNode::Var { .. } => unknown(), // local scope tracking is future work

            ExprNode::Let { value, body, .. } => {
                self.analyze_expr(value);
                self.analyze_expr(body)
            }

            ExprNode::Lambda { body, .. } => {
                self.analyze_expr(body);
                unknown() // Fn type synthesis is future work
            }

            ExprNode::Apply { fun, args, block } => {
                self.analyze_expr(fun);
                for a in args.iter_mut() { self.analyze_expr(a); }
                if let Some(b) = block { self.analyze_expr(b); }
                unknown()
            }

            ExprNode::Send { recv, method, args, block } => {
                let recv_ty = recv.as_mut().map(|r| self.analyze_expr(r));
                for a in args.iter_mut() { self.analyze_expr(a); }
                if let Some(b) = block { self.analyze_expr(b); }
                self.dispatch(recv_ty.as_ref(), method)
            }

            ExprNode::If { cond, then_branch, else_branch } => {
                self.analyze_expr(cond);
                let t = self.analyze_expr(then_branch);
                let e = self.analyze_expr(else_branch);
                union_of(t, e)
            }

            ExprNode::Case { scrutinee, arms } => {
                self.analyze_expr(scrutinee);
                let mut branch_tys = Vec::new();
                for arm in arms.iter_mut() {
                    if let Some(g) = &mut arm.guard { self.analyze_expr(g); }
                    branch_tys.push(self.analyze_expr(&mut arm.body));
                }
                union_many(branch_tys)
            }

            ExprNode::Seq { exprs } => {
                let mut last = Ty::Nil;
                for e in exprs.iter_mut() {
                    last = self.analyze_expr(e);
                }
                last
            }

            ExprNode::Assign { target, value } => {
                let value_ty = self.analyze_expr(value);
                // Annotate lvalue-side subexpressions that carry their own.
                if let LValue::Attr { recv, .. } = target {
                    self.analyze_expr(recv);
                }
                if let LValue::Index { recv, index } = target {
                    self.analyze_expr(recv);
                    self.analyze_expr(index);
                }
                value_ty
            }

            ExprNode::Yield { args } => {
                for a in args.iter_mut() { self.analyze_expr(a); }
                unknown()
            }

            ExprNode::Raise { value } => {
                self.analyze_expr(value);
                // `raise` produces no value; use Nil.
                Ty::Nil
            }
        }
    }

    fn dispatch(&self, recv_ty: Option<&Ty>, method: &Symbol) -> Ty {
        match recv_ty {
            None => unknown(), // implicit self; binding context tracking is future work
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
