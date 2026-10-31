//! `normalizes :email_address, with: ->(e) { e.strip.downcase }` (Rails
//! 7.1) — the declaration the Rails 8 authentication generator puts on
//! every `User`.
//!
//! Rails applies the normalization in two places, and both matter for a
//! sign-in form:
//!
//!   - on assignment: `User.new(email_address: " A@B.C ").email_address`
//!     is `"a@b.c"`, and so is what gets saved;
//!   - to the matching keyword of a finder: `User.find_by(email_address:
//!     " A@B.C ")` finds that row, which is what lets a user sign in with
//!     the capitalization they typed.
//!
//! The lambda becomes one synthesized class method,
//! `Model._normalize_<attr>(value)`, with the lambda's body inlined over
//! `value` and nil passed through (Rails' `apply_to_nil: false`
//! default). The column writer calls it, and the finder rewrite below
//! wraps the keyword's value in it — one copy of the body, and the
//! value expression evaluated once.
//!
//! DIVERGENCE: hydration assigns through the same column writer, so a
//! row loaded from the database is normalized too, where Rails leaves an
//! existing row's value alone until it is reassigned. The two agree for
//! every row written by the app after the declaration.
//!
//! `apply_to_nil: true` drops the nil pass-through: an assigned nil is
//! normalized too (`->(t) { t.to_s }` makes it ""). `normalize_value_for
//! (:attr, value)` on the model (a class method, `self.class.` in an
//! instance method, a scope) is the same call as the column writer's.
//!
//! Not reproduced, and left alone rather than half-applied: a `with:`
//! that is not a one-parameter lambda literal.

use crate::det::HashMap;

use indexmap::IndexMap;

use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// The normalizing class method's name for `attr`.
pub(crate) fn normalizer_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("_normalize_{}", attr.as_str()))
}

/// Every attribute `model` declares a reproducible normalization for,
/// with the lambda's parameter and body, in declaration order (an
/// `IndexMap`: a repeated `normalizes :attr` keeps its first position
/// but the later declaration's body wins, same as a Ruby `Hash`).
pub(crate) fn normalizations(model: &Model) -> IndexMap<Symbol, (Symbol, Expr, bool)> {
    let mut out = IndexMap::new();
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let Some((attrs, param, body, apply_to_nil)) = reproducible(expr) else { continue };
        for attr in attrs {
            out.insert(attr, (param.clone(), body.clone(), apply_to_nil));
        }
    }
    out
}

/// Is `expr` a `normalizes` call this lowering applies in full: one it
/// reproduces, every attribute of which is a column (the normalizer is
/// generated for columns only)? The model path claims exactly these;
/// any other `normalizes` stays a reported gap.
pub(crate) fn lowers_call(expr: &Expr, is_column: impl Fn(&Symbol) -> bool) -> bool {
    reproducible(expr).is_some_and(|(attrs, ..)| !attrs.is_empty() && attrs.iter().all(is_column))
}

/// A `normalizes :a, :b, with: ->(x) { … }, apply_to_nil: <bool>` call
/// with a one-parameter lambda literal and no other option: its
/// attributes, the lambda's parameter and body, and `apply_to_nil`.
fn reproducible(expr: &Expr) -> Option<(Vec<Symbol>, Symbol, Expr, bool)> {
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return None };
    if method.as_str() != "normalizes" {
        return None;
    }
    let mut attrs = Vec::new();
    let mut with = None;
    let mut apply_to_nil = false;
    for arg in args {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => attrs.push(value.clone()),
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
                    match (key.as_str(), &*v.node) {
                        ("with", ExprNode::Lambda { params, rest_param: None, extra_params, body, .. })
                            if params.len() == 1 && extra_params.is_empty() =>
                        {
                            with = Some((params[0].clone(), body.clone()));
                        }
                        ("apply_to_nil", ExprNode::Lit { value: Literal::Bool { value } }) => apply_to_nil = *value,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    let (param, body) = with?;
    Some((attrs, param, body, apply_to_nil))
}

/// `Model._normalize_<attr>(value)` for each normalized attribute that
/// is one of the model's columns: `value.nil? ? nil : <body>`, or the
/// body alone under `apply_to_nil: true`. With `cast` (the ruby family),
/// a String or Integer column's value is first cast by its type
/// (`ActiveRecord::Type::String#cast`), as Rails casts before it
/// normalizes: assignment, a finder keyword and `normalize_value_for`
/// all reach it through here.
pub(crate) fn push_normalize_methods(methods: &mut Vec<MethodDef>, model: &Model, columns: &HashMap<Symbol, Ty>, cast: bool) {
    let type_class = |ty: &Ty| match ty {
        Ty::Str => Some("String"),
        Ty::Int => Some("Integer"),
        Ty::Union { variants } => variants.iter().find_map(|v| match v {
            Ty::Str => Some("String"),
            Ty::Int => Some("Integer"),
            _ => None,
        }),
        _ => None,
    };
    for (attr, (param, body, apply_to_nil)) in normalizations(model) {
        let Some(col_ty) = columns.get(&attr) else { continue };
        let name = normalizer_name(&attr);
        if methods.iter().any(|m| m.receiver == MethodReceiver::Class && m.name == name) {
            continue;
        }
        let value = Symbol::from("value");
        let read_value = || synthetic(ExprNode::Var { id: VarId(0), name: value.clone() });
        let send = |recv: Expr, method: &str, args: Vec<Expr>| {
            synthetic(ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized: false })
        };
        let lambda_with = |input: Expr| {
            let mut normalized = body.clone();
            crate::lower::case_lambda::subst(&mut normalized, &param, &input);
            normalized
        };
        let nil_lit = || synthetic(ExprNode::Lit { value: Literal::Nil });
        let str_lit = |v: &str| synthetic(ExprNode::Lit { value: Literal::Str { value: v.to_string() } });
        let assign = |name: &Symbol, v: Expr| {
            synthetic(ExprNode::Assign { target: crate::expr::LValue::Var { id: VarId(0), name: name.clone() }, value: v })
        };
        let if_nil = |cond_on: Expr, then_branch: Expr, else_branch: Expr| {
            synthetic(ExprNode::If { cond: send(cond_on, "nil?", vec![]), then_branch, else_branch })
        };
        // The value the lambda sees is cast by the column's type, as Rails
        // casts it; nil stays nil:
        // - a String column's non-nil value goes into a String local
        //   (`true` → "t", `false` → "f", else `to_s`), which keeps what the
        //   lambda calls on it lowered (`squish`). Under `apply_to_nil` the
        //   nil branch runs the lambda on the nil itself.
        // - an Integer column's goes through
        //   `ActiveRecord::Type::Integer.new.cast(value)` (blank is nil).
        let cast_value = Symbol::from("cast_value");
        let read_cast = || synthetic(ExprNode::Var { id: VarId(0), name: cast_value.clone() });
        let body = match if cast { type_class(col_ty) } else { None } {
            Some("String") => {
                let as_string = synthetic(ExprNode::If {
                    cond: send(read_value(), "==", vec![synthetic(ExprNode::Lit { value: Literal::Bool { value: true } })]),
                    then_branch: str_lit("t"),
                    else_branch: synthetic(ExprNode::If {
                        cond: send(read_value(), "==", vec![synthetic(ExprNode::Lit { value: Literal::Bool { value: false } })]),
                        then_branch: str_lit("f"),
                        else_branch: send(read_value(), "to_s", vec![]),
                    }),
                });
                let present = synthetic(ExprNode::Seq { exprs: vec![assign(&cast_value, as_string), lambda_with(read_cast())] });
                let absent = if apply_to_nil { lambda_with(read_value()) } else { nil_lit() };
                if_nil(read_value(), absent, present)
            }
            Some(type_name) => {
                let type_object = send(
                    synthetic(ExprNode::Const {
                        path: vec![Symbol::from("ActiveRecord"), Symbol::from("Type"), Symbol::from(type_name)],
                    }),
                    "new",
                    vec![],
                );
                let mut cast_call = send(type_object, "cast", vec![read_value()]);
                if let ExprNode::Send { parenthesized, .. } = &mut *cast_call.node {
                    *parenthesized = true;
                }
                let normalized = lambda_with(read_cast());
                let guarded = if apply_to_nil { normalized } else { if_nil(read_cast(), nil_lit(), normalized) };
                synthetic(ExprNode::Seq { exprs: vec![assign(&cast_value, cast_call), guarded] })
            }
            None => {
                let normalized = lambda_with(read_value());
                if apply_to_nil { normalized } else { if_nil(read_value(), nil_lit(), normalized) }
            }
        };
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: Span::synthetic(),
            name,
            receiver: MethodReceiver::Class,
            params: vec![Param::positional(value.clone())],
            body,
            signature: Some(super::model_to_library::fn_sig(vec![(value, col_ty.clone())], col_ty.clone())),
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

/// The column writer's right-hand side for a normalized attribute:
/// `Model._normalize_<attr>(value)`.
pub(crate) fn normalized_write(model: &Model, attr: &Symbol, value: Expr) -> Option<Expr> {
    if !normalizations(model).contains_key(attr) {
        return None;
    }
    Some(call_normalizer(&model.name, attr, value))
}

fn call_normalizer(model: &ClassId, attr: &Symbol, value: Expr) -> Expr {
    // The model's constant, a synthesized class reference: no source span
    // (the value's would make the resolver look for a constant there).
    let mut model_ref = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: model.0.as_str().split("::").map(Symbol::from).collect() },
    );
    model_ref.decisions |= crate::expr::RESOLVED_CLASS_REF;
    let mut e = Expr::new(
        value.span,
        ExprNode::Send {
            recv: Some(model_ref),
            method: normalizer_name(attr),
            args: vec![value.clone()],
            block: None,
            parenthesized: true,
        },
    );
    e.ty = value.ty.clone();
    e
}

/// Finder keywords: `Model.find_by(email_address: v)` (and `find_by!`,
/// `where`, `exists?`, `find_or_create_by`, `find_or_initialize_by`, on
/// the class or a relation over it) normalize `v` the way assignment
/// would. Runs after every pass that synthesizes a `find_by`
/// (`authenticate_by` expands into one).
pub fn apply_normalizes_finder_lowering(app: &mut App) {
    // Only the attributes `push_normalize_methods` writes a normalizer
    // for: the model's columns. Another one (an `attr_accessor`) has no
    // `_normalize_<attr>` to call.
    let schema = &app.schema;
    let normalized: HashMap<ClassId, Vec<Symbol>> = app
        .models
        .iter()
        .filter_map(|m| {
            let table = schema.tables.get(&m.table.0);
            let attrs: Vec<Symbol> = normalizations(m)
                .into_keys()
                .filter(|a| table.is_some_and(|t| t.columns.iter().any(|c| &c.name == a)))
                .collect();
            (!attrs.is_empty()).then(|| (m.name.clone(), attrs))
        })
        .collect();
    if normalized.is_empty() {
        return;
    }
    super::for_each_owned_hook_body(app, &mut |owner, body| rewrite(body, &normalized, owner));
}

const FINDERS: &[&str] =
    &["find_by", "find_by!", "where", "exists?", "find_or_create_by", "find_or_initialize_by", "create_or_find_by", "create_or_find_by!"];

fn rewrite(e: &mut Expr, normalized: &HashMap<ClassId, Vec<Symbol>>, owner: Option<&ClassId>) {
    e.node.for_each_child_mut(&mut |c| rewrite(c, normalized, owner));
    if let Some(call) = normalize_value_for(e, normalized, owner) {
        *e = call;
        return;
    }
    let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *e.node else { return };
    if !FINDERS.contains(&method.as_str()) {
        return;
    }
    let class = match r.ty.as_ref() {
        Some(Ty::Class { id, .. }) => id.clone(),
        Some(Ty::Relation { of }) => of.clone(),
        _ => match &*r.node {
            ExprNode::Const { path } => {
                ClassId(Symbol::from(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")))
            }
            _ => return,
        },
    };
    let Some(attrs) = normalized.get(&class) else { return };
    for arg in args.iter_mut() {
        let ExprNode::Hash { entries, .. } = &mut *arg.node else { continue };
        for (k, v) in entries.iter_mut() {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { continue };
            if !attrs.contains(key) || is_normalizer_call(v) {
                continue;
            }
            *v = normalize_condition(&class, key, v.clone());
        }
    }
}

/// `normalize_value_for(:attr, value)` on a model that normalizes
/// `attr` — receiver-less or `self.` in its own class methods and
/// scopes, `self.class.` in its instance methods, or the model's
/// constant — as `Model._normalize_<attr>(value)`. Another attribute is
/// left alone (Rails casts it by its type there).
fn normalize_value_for(e: &Expr, normalized: &HashMap<ClassId, Vec<Symbol>>, owner: Option<&ClassId>) -> Option<Expr> {
    let ExprNode::Send { recv, method, args, .. } = &*e.node else { return None };
    if method.as_str() != "normalize_value_for" || args.len() != 2 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Sym { value: attr } } = &*args[0].node else { return None };
    let class = match recv.as_ref().map(|r| &*r.node) {
        None | Some(ExprNode::SelfRef) => owner?.clone(),
        Some(ExprNode::Send { recv: Some(inner), method, args, .. })
            if method.as_str() == "class" && args.is_empty() && matches!(&*inner.node, ExprNode::SelfRef) =>
        {
            owner?.clone()
        }
        Some(ExprNode::Const { path }) => {
            ClassId(Symbol::from(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")))
        }
        _ => return None,
    };
    if !normalized.get(&class)?.contains(attr) {
        return None;
    }
    let mut call = call_normalizer(&class, attr, args[1].clone());
    call.span = e.span;
    Some(call)
}

/// Already wrapped — the rewrite must be idempotent, since a body can be
/// visited twice (a concern's copy and its includer's).
/// A finder condition's value as Rails normalizes it: a scalar through
/// the attribute's normalizer, an Array element by element. A nil
/// element stays nil, since a finder never normalizes nil, not even
/// under `apply_to_nil: true` (`where(note: [nil])` is `"note" IS
/// NULL`). Before, the whole Array went to the normalizer, which
/// normalized its `to_s`.
fn normalize_condition(model: &ClassId, attr: &Symbol, value: Expr) -> Expr {
    if let ExprNode::Array { elements, style } = &*value.node
        && !elements.iter().any(|e| matches!(&*e.node, ExprNode::Splat { .. }))
    {
        let elements = elements
            .iter()
            .map(|e| {
                if matches!(&*e.node, ExprNode::Lit { value: Literal::Nil }) || is_normalizer_call(e) {
                    e.clone()
                } else {
                    call_normalizer(model, attr, e.clone())
                }
            })
            .collect();
        let mut array = Expr::new(value.span, ExprNode::Array { elements, style: style.clone() });
        array.ty = value.ty.clone();
        return array;
    }
    let Some(Ty::Array { elem }) = value.ty.clone() else {
        return call_normalizer(model, attr, value);
    };
    // `titles.map { |element| Model._normalize_title(element) }`, with
    // a nil guard when an element can be nil.
    let span = value.span;
    let name = Symbol::from("element");
    let mut read = Expr::new(span, ExprNode::Var { id: VarId(0), name: name.clone() });
    read.ty = Some(elem.as_ref().clone());
    let normalized = call_normalizer(model, attr, read.clone());
    let can_be_nil = match elem.as_ref() {
        Ty::Nil | Ty::Untyped => true,
        Ty::Union { variants } => variants.contains(&Ty::Nil),
        _ => false,
    };
    let body = if can_be_nil {
        let mut nil_check = Expr::new(
            span,
            ExprNode::Send { recv: Some(read), method: Symbol::from("nil?"), args: vec![], block: None, parenthesized: false },
        );
        nil_check.ty = Some(Ty::Bool);
        let mut if_ = Expr::new(
            span,
            ExprNode::If {
                cond: nil_check,
                then_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                else_branch: normalized,
            },
        );
        if_.ty = Some(elem.as_ref().clone());
        if_
    } else {
        normalized
    };
    let block = Expr::new(
        span,
        ExprNode::Lambda {
            extra_params: Vec::new(),
            params: vec![name],
            rest_param: None,
            block_param: None,
            body,
            block_style: crate::expr::BlockStyle::Brace,
        },
    );
    let ty = value.ty.clone();
    let mut map = Expr::new(
        span,
        ExprNode::Send { recv: Some(value), method: Symbol::from("map"), args: vec![], block: Some(block), parenthesized: false },
    );
    map.ty = ty;
    map
}

fn is_normalizer_call(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str().starts_with("_normalize_"))
}

fn synthetic(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}
