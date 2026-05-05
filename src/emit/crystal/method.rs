//! Crystal `def` emission with type annotations.
//!
//! Drives off `MethodDef.signature: Option<Ty::Fn>`. When the signature
//! is fully typed (no `Untyped` reachable through params/return), emit
//! the annotated form `def name(a : T, b : U) : R`. When any position
//! is `Untyped`, drop annotations entirely and let Crystal's inference
//! fill in — partial annotation triggers Crystal's stricter checking
//! and would surface false-positive errors at the gap.

use std::fmt::Write;

use super::expr::emit_expr;
use super::ty::{crystal_ty, has_untyped};
use crate::dialect::{MethodDef, MethodReceiver};
use crate::ty::Ty;

/// Emit a single `MethodDef` as Crystal source (trailing newline
/// included). Mirrors `super::super::ruby::emit_method` in surface
/// shape; adds Crystal-specific signature annotations.
pub fn emit_method(m: &MethodDef) -> String {
    let prefix = match m.receiver {
        MethodReceiver::Instance => "",
        MethodReceiver::Class => "self.",
    };

    // Decide whether to emit type annotations. The signature is the
    // authority; when missing or carrying `Untyped`, fall back to
    // bare `def name(args)`.
    let annotate = m
        .signature
        .as_ref()
        .map(|sig| !sig_has_untyped(sig))
        .unwrap_or(false);

    let params = render_params(m, annotate);
    let ret_clause = if annotate {
        if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
            format!(" : {}", crystal_ty(ret))
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut out = String::new();
    writeln!(out, "def {prefix}{}{params}{ret_clause}", m.name).unwrap();
    let body_text = emit_expr(&m.body);
    for line in body_text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("end\n");
    out
}

fn render_params(m: &MethodDef, annotate: bool) -> String {
    if m.params.is_empty() {
        return String::new();
    }
    let sig_params = if annotate {
        if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
            Some(params)
        } else {
            None
        }
    } else {
        None
    };

    let ps: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p.name.as_str();
            let default_clause = match &p.default {
                Some(default) => format!(" = {}", emit_expr(default)),
                None => String::new(),
            };
            match sig_params.as_ref().and_then(|sp| sp.get(i)) {
                Some(sig_p) => format!("{name} : {}{default_clause}", crystal_ty(&sig_p.ty)),
                None => format!("{name}{default_clause}"),
            }
        })
        .collect();
    format!("({})", ps.join(", "))
}

fn sig_has_untyped(sig: &Ty) -> bool {
    let Ty::Fn { params, ret, .. } = sig else {
        return true;
    };
    params.iter().any(|p| has_untyped(&p.ty)) || has_untyped(ret)
}
