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

/// Receiver-kind picker driven by the `mutates_self` heuristic
/// computed in `library.rs`. Returned tokens already include the
/// trailing comma when a method has additional params, so callers
/// can splice without re-checking emptiness.
fn render_self_receiver(mutates: bool) -> &'static str {
    if mutates { "&mut self" } else { "&self" }
}

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

/// Emit a single instance method. `mutates_self` decides the
/// receiver token (`&self` vs `&mut self`). `def initialize` is
/// special-cased to `pub fn new(...) -> Self` — Rust constructors
/// don't take a self receiver and return the constructed value.
///
/// The `_struct_name` and `_ivars` params are passed for the
/// `initialize → new` body synthesis (next commit's scope): the
/// constructor needs to render the user's body then close with a
/// `Self { f1, f2, .. }` literal initialized from the ivars. For
/// now they're unused — `initialize` emits the body as-is and the
/// resulting Rust will fail to compile until that synthesis lands.
pub(super) fn emit_instance_method(
    m: &MethodDef,
    mutates_self: bool,
    _struct_name: &str,
    ivars: &[(String, Ty)],
) -> Result<String, String> {
    if !matches!(m.receiver, MethodReceiver::Instance) {
        return Err(format!(
            "rust2::emit_instance_method: expected instance method, got class method `{}`",
            m.name
        ));
    }
    let mut out = String::new();
    let is_init = m.name.as_str() == "initialize";
    let sanitized = super::expr::sanitize_ident(m.name.as_str());
    let (fn_name, receiver): (&str, Option<&'static str>) = if is_init {
        ("new", None)
    } else {
        (sanitized.as_str(), Some(render_self_receiver(mutates_self)))
    };
    let params = render_instance_params(m, receiver);
    let ret_clause = if is_init {
        " -> Self".to_string()
    } else {
        render_return(m)
    };
    writeln!(out, "pub fn {fn_name}{params}{ret_clause} {{").unwrap();
    let body = if is_init {
        super::expr::with_constructor_mode(|| emit_expr(&m.body))
    } else {
        emit_expr(&m.body)
    };
    for line in body.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    if is_init {
        // Close the constructor with `Self { f1, f2, ... }` — Rust's
        // struct-literal shorthand binds field names to local
        // variables of the same name, which is precisely what the
        // ivar→local rewrite above produces. Empty-ivar classes get
        // `Self {}` (still valid).
        let fields: Vec<&str> = ivars.iter().map(|(n, _)| n.as_str()).collect();
        writeln!(out, "    Self {{ {} }}", fields.join(", ")).unwrap();
    }
    out.push_str("}\n");
    Ok(out)
}

fn render_instance_params(m: &MethodDef, receiver: Option<&'static str>) -> String {
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) if params.len() == m.params.len() => Some(params),
        _ => None,
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = receiver {
        parts.push(r.to_string());
    }
    for (i, p) in m.params.iter().enumerate() {
        let name = p.name.as_str();
        let rendered = match sig_params.and_then(|sp| sp.get(i)) {
            Some(sig_p) => format!("{name}: {}", rust_param_ty(&sig_p.ty)),
            None => format!("{name}: ()"),
        };
        parts.push(rendered);
    }
    format!("({})", parts.join(", "))
}
