//! `render json: <value>` whose value holds a record, a Time or a Date — a Hash or Array literal, a typed collection — written down from its static type as the JSON text Rails' `to_json` answers.

use std::collections::HashSet;
use std::sync::Arc;

use crate::expr::{BlockStyle, Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::ty::Ty;

use super::as_json_poro::{collection_text, writer_call};
use super::typing::with_ty;

/// Whether `value` reaches a record with a writer, a Time or a Date — what the primitive `JSON.generate` path cannot encode.
pub(super) fn needs_text(value: &Expr, written: &HashSet<ClassId>) -> bool {
    match &*value.node {
        ExprNode::Hash { entries, .. } => entries.iter().any(|(_, v)| needs_text(v, written)),
        ExprNode::Array { elements, .. } => elements.iter().any(|e| needs_text(e, written)),
        ExprNode::Lit { .. } => false,
        _ => value.ty.as_ref().is_some_and(|t| ty_needs(t, written)),
    }
}

fn ty_needs(ty: &Ty, written: &HashSet<ClassId>) -> bool {
    match ty {
        Ty::Time | Ty::Date => true,
        Ty::Class { id, .. } | Ty::Relation { of: id } => written.contains(id),
        Ty::Array { elem } => ty_needs(elem, written),
        Ty::Hash { value, .. } => ty_needs(value, written),
        Ty::Union { variants } => variants.iter().any(|v| ty_needs(v, written)),
        _ => false,
    }
}

/// The classes a `render json:` value serializes anywhere inside it, for the demand gate.
pub(super) fn classes_in(value: &Expr, out: &mut Vec<ClassId>) {
    match &*value.node {
        ExprNode::Hash { entries, .. } => entries.iter().for_each(|(_, v)| classes_in(v, out)),
        ExprNode::Array { elements, .. } => elements.iter().for_each(|e| classes_in(e, out)),
        _ => {
            if let Some(ty) = &value.ty {
                ty_classes(ty, out);
            }
        }
    }
}

fn ty_classes(ty: &Ty, out: &mut Vec<ClassId>) {
    match ty {
        Ty::Class { id, .. } | Ty::Relation { of: id } => out.push(id.clone()),
        Ty::Array { elem } => ty_classes(elem, out),
        Ty::Hash { value, .. } => ty_classes(value, out),
        Ty::Union { variants } => variants.iter().for_each(|v| ty_classes(v, out)),
        _ => {}
    }
}

/// The JSON text of `value` for a site, or `None` when some part has no encoding here (the site then keeps the runtime encoder).
pub(super) fn json_text(value: &Expr, written: &HashSet<ClassId>) -> Option<Expr> {
    let text = part_text(value, written)?;
    Some(call(value, &["ActiveSupport", "JSON"], "written", text))
}

fn part_text(value: &Expr, written: &HashSet<ClassId>) -> Option<Expr> {
    match &*value.node {
        ExprNode::Lit { value: Literal::Nil } => Some(lit(value, "null")),
        ExprNode::Hash { entries, .. } => {
            let mut parts = vec![lit(value, "{")];
            for (i, (k, v)) in entries.iter().enumerate() {
                let key = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    _ => return None,
                };
                // Not escaped here: a key needing JSON escapes declines rather than spelling an escape table twice.
                if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    return None;
                }
                parts.push(lit(value, &format!("{}\"{}\":", if i == 0 { "" } else { "," }, key)));
                parts.push(part_text(v, written)?);
            }
            parts.push(lit(value, "}"));
            Some(concat(parts))
        }
        ExprNode::Array { elements, .. } => {
            let mut parts = vec![lit(value, "[")];
            for (i, e) in elements.iter().enumerate() {
                // Not written down: a splat's elements are only known at run time.
                if matches!(&*e.node, ExprNode::Splat { .. }) {
                    return None;
                }
                if i > 0 {
                    parts.push(lit(value, ","));
                }
                parts.push(part_text(e, written)?);
            }
            parts.push(lit(value, "]"));
            Some(concat(parts))
        }
        _ => ty_text(value, value.ty.as_ref()?, written),
    }
}

fn ty_text(value: &Expr, ty: &Ty, written: &HashSet<ClassId>) -> Option<Expr> {
    if scalar(ty) {
        return Some(call(value, &["JsonBuilder"], "encode_value", value.clone()));
    }
    match ty {
        Ty::Time => Some(call(value, &["ActiveSupport", "JSON"], "time_fragment", value.clone())),
        Ty::Union { variants } if variants.iter().all(|v| matches!(v, Ty::Time | Ty::Nil)) => {
            Some(call(value, &["ActiveSupport", "JSON"], "time_fragment", value.clone()))
        }
        // `format_db_date` answers nil for nil, which `encode_value` writes as `null`.
        Ty::Date => Some(date_text(value)),
        Ty::Union { variants } if variants.iter().all(|v| matches!(v, Ty::Date | Ty::Nil)) => Some(date_text(value)),
        Ty::Class { id, .. } if written.contains(id) => Some(writer_call(value.clone())),
        // Only a variable read: the value is read twice, once for the nil test.
        Ty::Union { variants } if matches!(&*value.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) => {
            let mut others = variants.iter().filter(|v| !matches!(v, Ty::Nil));
            let (Some(Ty::Class { id, .. }), None) = (others.next(), others.next()) else { return None };
            if !written.contains(id) || !variants.iter().any(|v| matches!(v, Ty::Nil)) {
                return None;
            }
            let present = with_ty(value.clone(), Ty::Class { id: id.clone(), args: vec![].into() });
            Some(with_ty(
                Expr::new(
                    value.span,
                    ExprNode::If {
                        cond: send(Some(value.clone()), "nil?", vec![], Ty::Bool),
                        then_branch: lit(value, "null"),
                        else_branch: writer_call(present),
                    },
                ),
                Ty::Str,
            ))
        }
        Ty::Relation { of } if written.contains(of) => Some(collection_text(value, of)),
        Ty::Array { elem } => {
            if let Ty::Class { id, .. } = &**elem {
                return written.contains(id).then(|| collection_text(value, id));
            }
            let item = var(value, "item", (**elem).clone());
            let body = ty_text(&item, elem, written)?;
            let mapped = map_join(value, vec![Symbol::from("item")], body);
            Some(concat(vec![lit(value, "["), mapped, lit(value, "]")]))
        }
        Ty::Hash { key, value: v } if matches!(**key, Ty::Str | Ty::Sym) => {
            let k = var(value, "key", (**key).clone());
            let item = var(value, "item", (**v).clone());
            let key_text = call(value, &["JsonBuilder"], "encode_value", send(Some(k), "to_s", vec![], Ty::Str));
            let body = concat(vec![key_text, lit(value, ":"), ty_text(&item, v, written)?]);
            let mapped = map_join(value, vec![Symbol::from("key"), Symbol::from("item")], body);
            Some(concat(vec![lit(value, "{"), mapped, lit(value, "}")]))
        }
        _ => None,
    }
}

fn scalar(ty: &Ty) -> bool {
    match ty {
        Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool | Ty::Nil => true,
        Ty::Union { variants } => variants.iter().all(scalar),
        _ => false,
    }
}

fn date_text(value: &Expr) -> Expr {
    let text = with_ty(
        send(Some(const_ref(value, &["ActiveSupport"])), "format_db_date", vec![value.clone()], Ty::Union {
            variants: vec![Ty::Str, Ty::Nil].into(),
        }),
        Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() },
    );
    call(value, &["JsonBuilder"], "encode_value", text)
}

/// `<value>.map { |params| <body> }.join(",")`
fn map_join(value: &Expr, params: Vec<Symbol>, body: Expr) -> Expr {
    let block = Expr::new(
        value.span,
        ExprNode::Lambda {
            extra_params: Vec::new(),
            rest_param: None,
            params,
            block_param: None,
            body,
            block_style: BlockStyle::Brace,
        },
    );
    let mapped = with_ty(
        Expr::new(
            value.span,
            ExprNode::Send {
                recv: Some(value.clone()),
                method: Symbol::from("map"),
                args: vec![],
                block: Some(block),
                parenthesized: true,
            },
        ),
        Ty::Array { elem: Arc::new(Ty::Str) },
    );
    send(Some(mapped), "join", vec![lit(value, ",")], Ty::Str)
}

fn concat(parts: Vec<Expr>) -> Expr {
    let mut it = parts.into_iter();
    let first = it.next().expect("at least one part");
    it.fold(first, |acc, p| send(Some(acc), "+", vec![p], Ty::Str))
}

fn call(at: &Expr, path: &[&str], method: &str, arg: Expr) -> Expr {
    send(Some(const_ref(at, path)), method, vec![arg], Ty::Str)
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>, ty: Ty) -> Expr {
    let span = recv.as_ref().map(|r| r.span).or_else(|| args.first().map(|a| a.span)).unwrap_or_else(crate::span::Span::synthetic);
    with_ty(
        Expr::new(
            span,
            ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
        ),
        ty,
    )
}

fn const_ref(at: &Expr, path: &[&str]) -> Expr {
    Expr::new(at.span, ExprNode::Const { path: path.iter().map(|p| Symbol::from(*p)).collect() })
}

fn var(at: &Expr, name: &str, ty: Ty) -> Expr {
    with_ty(Expr::new(at.span, ExprNode::Var { id: VarId(0), name: Symbol::from(name) }), ty)
}

fn lit(at: &Expr, s: &str) -> Expr {
    with_ty(Expr::new(at.span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } }), Ty::Str)
}
