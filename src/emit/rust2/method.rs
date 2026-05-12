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
    let fn_name = super::expr::sanitize_ident(m.name.as_str());
    writeln!(out, "pub fn {fn_name}{params}{ret_clause} {{").unwrap();
    let body = super::expr::with_method_scope(&m.body, || emit_expr(&m.body));
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
    is_static: bool,
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
    // `pub fn new(...)` constructors and static-safe methods both
    // drop the `&self` receiver — the latter were identified by
    // `library.rs::method_reads_self`. Call sites for static methods
    // route through `Self::method(args)` (see expr.rs::emit_send).
    let (fn_name, receiver): (&str, Option<&'static str>) = if is_init {
        ("new", None)
    } else if is_static {
        (sanitized.as_str(), None)
    } else {
        (sanitized.as_str(), Some(render_self_receiver(mutates_self)))
    };
    // Block-param injection: methods whose body uses `yield` get
    // an explicit closure parameter `mut f: impl FnMut(...)`. The
    // body's `yield x, y` sites then emit as `f(x, y)`.
    //
    // Arity + types come from the first observed Yield call in the
    // body. The RBS signature carries a `block: Option<Box<Ty>>`
    // slot, but the current RBS parser collapses block signatures
    // to `Ty::Untyped` — until that improves, body-derived
    // inference is the only signal. HWIA's `each` body yields
    // `(k: String, v: serde_json::Value)`, which is what we want.
    let block_param = find_yield_signature(&m.body).map(render_block_param_from_args);
    let params = render_instance_params(m, receiver, block_param.as_deref());
    let ret_clause = if is_init {
        " -> Self".to_string()
    } else {
        render_return(m)
    };
    writeln!(out, "pub fn {fn_name}{params}{ret_clause} {{").unwrap();
    let body = super::expr::with_method_scope(&m.body, || {
        if is_init {
            let fields: Vec<String> = ivars.iter().map(|(n, _)| n.clone()).collect();
            super::expr::with_constructor_mode(fields, || emit_expr(&m.body))
        } else {
            emit_expr(&m.body)
        }
    });
    let body_lines: Vec<&str> = body.lines().collect();
    let last_idx = body_lines.len().saturating_sub(1);
    for (i, line) in body_lines.iter().enumerate() {
        if line.is_empty() {
            writeln!(out).unwrap();
            continue;
        }
        // In constructor mode the user body's tail is followed by
        // a `Self { ... }` literal that must be a separate statement —
        // the body's `Seq` emit leaves the tail un-terminated (Rust's
        // block-value convention), which would concatenate the tail
        // with the Self literal. Force-terminate the last user-body
        // line for the constructor case so the appended Self literal
        // is a distinct expression on its own line.
        let needs_terminator = is_init
            && i == last_idx
            && !line.trim_end().ends_with(';')
            && !line.trim_end().ends_with('{')
            && !line.trim_end().ends_with('}');
        if needs_terminator {
            writeln!(out, "    {line};").unwrap();
        } else {
            writeln!(out, "    {line}").unwrap();
        }
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

fn render_instance_params(
    m: &MethodDef,
    receiver: Option<&'static str>,
    block_param: Option<&str>,
) -> String {
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
    if let Some(bp) = block_param {
        parts.push(bp.to_string());
    }
    format!("({})", parts.join(", "))
}

/// Render the closure-typed param for a yield-using method.
/// `block_ty` is the Ty::Fn from the RBS signature's block slot.
/// Emits `mut f: impl FnMut(P1, P2, ...)` — `mut` so the closure
/// can be called repeatedly inside the body (Yield in a loop is the
/// common case), `impl FnMut` so the call site can pass any closure
/// matching the param types without forcing a generic on the type.
/// Walk a method body for the first `Yield { args }` site and
/// return its arg types. `None` means no Yield in the body — caller
/// skips block-param injection entirely. Body-derived signature
/// is consistent for a given method: Ruby's `yield a, b` shape
/// doesn't vary across call sites of the same method, so first-hit
/// arity is authoritative.
fn find_yield_signature(body: &crate::expr::Expr) -> Option<Vec<Ty>> {
    use crate::expr::ExprNode;
    match &*body.node {
        ExprNode::Yield { args } => {
            Some(args.iter().map(|a| a.ty.clone().unwrap_or(Ty::Untyped)).collect())
        }
        ExprNode::Seq { exprs } => exprs.iter().find_map(find_yield_signature),
        ExprNode::If { cond, then_branch, else_branch } => find_yield_signature(cond)
            .or_else(|| find_yield_signature(then_branch))
            .or_else(|| find_yield_signature(else_branch)),
        ExprNode::While { cond, body, .. } => {
            find_yield_signature(cond).or_else(|| find_yield_signature(body))
        }
        ExprNode::Send { recv, args, block, .. } => recv
            .as_ref()
            .and_then(find_yield_signature)
            .or_else(|| args.iter().find_map(find_yield_signature))
            .or_else(|| block.as_ref().and_then(find_yield_signature)),
        ExprNode::Assign { value, .. } => find_yield_signature(value),
        ExprNode::Return { value } => find_yield_signature(value),
        _ => None,
    }
}

/// Render `mut f: impl FnMut(P1, P2, ...)` from the inferred arg
/// types. Empty arg list emits `mut f: impl FnMut()`. `mut` is
/// uniformly applied so calling the closure inside a loop (the
/// typical case) compiles without per-caller `mut` annotations.
fn render_block_param_from_args(arg_tys: Vec<Ty>) -> String {
    let ps: Vec<String> = arg_tys.iter().map(rust_param_ty).collect();
    format!("mut f: impl FnMut({})", ps.join(", "))
}
