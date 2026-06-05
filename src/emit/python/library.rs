//! Framework-runtime transpile: emit a lowered `LibraryClass` (or a
//! bare module of class-methods) as Python.
//!
//! Python sibling of `emit::typescript::{emit_library_class, emit_module}`.
//! It lets `runtime_loader::python_units` transpile the `runtime/ruby/*`
//! framework files (inflector, router, flash, …) into `app/*.py`,
//! strangling the hand-maintained `runtime/python/*.py` duplicates one
//! file at a time.
//!
//! Bodies route through the existing `expr::emit_body` walker: Python
//! needs no functionalize pass (it is mutable + imperative), so the same
//! walker the app-side model/controller emit uses covers framework method
//! bodies directly — the structural reason this is lighter than Elixir's
//! equivalent was to build.

use std::fmt::Write;

use super::expr::{emit_body, emit_expr};
use super::shared::indent_py;
use super::ty::python_ty;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::Expr;
use crate::ty::Ty;

/// Render a method's parameter list and return type. Prefers the
/// RBS-derived signature (populated by `parse_library_with_rbs`);
/// falls back to bare, un-annotated names when a method carries none.
fn params_and_ret(m: &MethodDef) -> (Vec<String>, Ty) {
    match &m.signature {
        Some(Ty::Fn { params, ret, .. }) if params.len() == m.params.len() => {
            let ps = m
                .params
                .iter()
                .zip(params.iter())
                .map(|(name, p)| format!("{}: {}", name, python_ty(&p.ty)))
                .collect();
            (ps, (**ret).clone())
        }
        _ => (
            m.params.iter().map(|p| p.to_string()).collect(),
            Ty::Untyped,
        ),
    }
}

/// Append `body`, indented one Python level (4 spaces), to `out`.
fn push_indented_body(out: &mut String, body: &str) {
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "    {line}").unwrap();
        }
    }
}

/// One method inside a class body: optional `@classmethod` decorator,
/// the `def` line with a `self`/`cls` leader and typed params, then the
/// indented body.
fn emit_class_method(m: &MethodDef) -> String {
    let mut out = String::new();
    let leader = match m.receiver {
        MethodReceiver::Instance => "self",
        MethodReceiver::Class => {
            writeln!(out, "@classmethod").unwrap();
            "cls"
        }
    };
    let (params, ret_ty) = params_and_ret(m);
    let mut sig = vec![leader.to_string()];
    sig.extend(params);
    writeln!(out, "def {}({}) -> {}:", m.name, sig.join(", "), python_ty(&ret_ty)).unwrap();
    push_indented_body(&mut out, &emit_body(&m.body, &ret_ty));
    out
}

/// Emit a lowered `LibraryClass` as a Python class declaration.
pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    let name = class.name.0.as_str();
    let mut out = String::new();

    // Parent + `include`d mixins both become Python base classes.
    let mut bases: Vec<String> = Vec::new();
    if let Some(parent) = &class.parent {
        bases.push(parent.0.as_str().to_string());
    }
    for inc in &class.includes {
        bases.push(inc.0.as_str().to_string());
    }
    if bases.is_empty() {
        writeln!(out, "class {name}:").unwrap();
    } else {
        writeln!(out, "class {name}({}):", bases.join(", ")).unwrap();
    }

    if class.methods.is_empty() {
        writeln!(out, "    pass").unwrap();
        return Ok(out);
    }
    for (i, m) in class.methods.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&indent_py(&emit_class_method(m)));
        out.push('\n');
    }
    Ok(out)
}

/// Emit a bare module (no enclosing class) of class-methods as top-level
/// Python functions. Used for `inflector.rb` / `json_builder.rb`, whose
/// `def self.x` methods become module-level `def x`.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    let mut out = String::new();
    for (i, m) in methods.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (params, ret_ty) = params_and_ret(m);
        writeln!(out, "def {}({}) -> {}:", m.name, params.join(", "), python_ty(&ret_ty)).unwrap();
        push_indented_body(&mut out, &emit_body(&m.body, &ret_ty));
    }
    Ok(out)
}

/// Render an `Expr` as a Python value expression — the `format_constant`
/// hook in `runtime_loader` uses this for module-level constants
/// (`STATUS_CODES = {...}`).
pub fn emit_expr_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}
