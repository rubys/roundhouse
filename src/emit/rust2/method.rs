//! `rust2` method emit — `MethodDef` → Rust `fn` rendering.
//!
//! Phase 2.1 scope: emit a `pub fn` for class methods (the Module-
//! mode runtime files like `inflector.rb`). Instance methods + per-
//! type signature handling come in Phase 2.2+ as struct emit lands.

use std::fmt::Write;

use super::expr::emit_expr;
use super::ty::rust_ty;
use crate::dialect::{MethodDef, MethodReceiver};
use crate::ty::Ty;

/// Emit a single `MethodDef` as a `pub fn` Rust function.
///
/// Module-mode methods (every `def self.X` in `inflector.rb`,
/// `view_helpers.rb`, etc.) become free `pub fn`s at the file's
/// module scope. The file IS the namespace (Rust convention),
/// so no extra `mod NAME { ... }` wrapping is needed — callers use
/// `crate::inflector::pluralize(...)`.
pub(super) fn emit_module_method(m: &MethodDef) -> Result<String, String> {
    if !matches!(m.receiver, MethodReceiver::Class) {
        return Err(format!(
            "rust2::emit_module_method: only class methods supported in Module mode, \
             got instance method `{}`",
            m.name
        ));
    }
    let mut out = String::new();
    let params = render_params(m);
    let ret_clause = render_return(m);
    writeln!(out, "pub fn {}{}{} {{", m.name.as_str(), params, ret_clause).unwrap();
    let body = emit_expr(&m.body);
    for line in body.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    out.push_str("}\n");
    Ok(out)
}

fn render_params(m: &MethodDef) -> String {
    if m.params.is_empty() {
        return "()".to_string();
    }
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) if params.len() == m.params.len() => Some(params),
        _ => None,
    };
    let parts: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p.name.as_str();
            match sig_params.and_then(|sp| sp.get(i)) {
                Some(sig_p) => format!("{name}: {}", rust_param_ty(&sig_p.ty)),
                None => format!("{name}: ()"),
            }
        })
        .collect();
    format!("({})", parts.join(", "))
}

fn render_return(m: &MethodDef) -> String {
    match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => {
            if matches!(&**ret, Ty::Nil) {
                String::new()
            } else {
                format!(" -> {}", rust_ty(ret))
            }
        }
        _ => String::new(),
    }
}

/// Parameter type rendering — borrow-aware variant of `rust_ty`.
/// String params take `&str` (avoids forcing callers to clone), Vec
/// stays owned for now (Phase 2.1 scope; refine at Phase 4 when
/// closures + lifetimes pressure surfaces).
fn rust_param_ty(ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => "&str".to_string(),
        other => rust_ty(other),
    }
}
