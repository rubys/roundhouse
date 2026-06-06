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
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::Expr;
use crate::ty::Ty;

/// Python is a flat-module target: a `Foo::Bar` library class emits as a
/// top-level `Bar`, with cross-file references wired through
/// `from app.x import Bar`. Drop any namespace to the last segment.
/// Mirrors TS's `rsplit("::")` at the class-decl site.
fn last_segment(qualified: &str) -> &str {
    qualified.rsplit("::").next().unwrap_or(qualified)
}

/// Map a Ruby parent class to its Python equivalent. Ruby's exception
/// root for application errors is `StandardError`; Python has no such
/// class — `Exception` is the equivalent base. Other names pass through
/// as their last namespace segment.
fn python_base_class(qualified: &str) -> String {
    match last_segment(qualified) {
        "StandardError" => "Exception".to_string(),
        other => other.to_string(),
    }
}

/// True for the synthetic reader/writer methods `attr_accessor` /
/// `attr_reader` / `attr_writer` lower to. Python models these as plain
/// instance attributes, so they emit as class-level annotated fields
/// rather than `def`s (`def notice=` isn't valid Python anyway).
fn is_accessor(m: &MethodDef) -> bool {
    matches!(m.kind, AccessorKind::AttributeReader | AccessorKind::AttributeWriter)
}

/// The field type for an accessor: a reader's return type or a writer's
/// sole-parameter type, falling back to the body's inferred type.
fn accessor_field_ty(m: &MethodDef) -> Ty {
    match (&m.kind, &m.signature) {
        (AccessorKind::AttributeReader, Some(Ty::Fn { ret, .. })) => (**ret).clone(),
        (AccessorKind::AttributeWriter, Some(Ty::Fn { params, .. })) if !params.is_empty() => {
            params[0].ty.clone()
        }
        _ => m.body.ty.clone().unwrap_or(Ty::Untyped),
    }
}

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

/// Append `body`, indented one Python level (4 spaces), to `out`. An
/// empty body (intentional in the framework — e.g. `def assign_from_row;
/// end`, whose override is supplied per-model) becomes `pass` so the
/// `def` isn't a syntax error.
fn push_indented_body(out: &mut String, body: &str) {
    if body.trim().is_empty() {
        out.push_str("    pass\n");
        return;
    }
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
    let py_name = super::shared::py_method_name(m.name.as_str());
    let mut sig = vec![leader.to_string()];
    sig.extend(params);
    // A method that `yield`s gets an injected `_block` parameter; the
    // body's `yield(...)` renders as `_block(...)` (see `emit::python::
    // expr`'s Yield arm). Mirrors the TS emitter's `__block`.
    if super::expr::body_contains_yield(&m.body) {
        sig.push("_block".to_string());
    }
    writeln!(out, "def {}({}) -> {}:", py_name, sig.join(", "), python_ty(&ret_ty)).unwrap();
    // `SelfRef` inside the body renders as the leader (`self`/`cls`) — a
    // lowering injects explicit self-receivers for implicit-self calls,
    // so a classmethod's `table_name` must reach `cls.table_name()`. A
    // `super(args)` in the body renders as `super().<py_name>(args)`.
    let body = super::expr::with_self_ref(leader, || {
        super::expr::with_self_sends(true, || {
            super::expr::with_super_method(&py_name, || emit_body(&m.body, &ret_ty))
        })
    });
    push_indented_body(&mut out, &body);
    out
}

/// Emit a lowered `LibraryClass` as a Python class declaration.
pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    let name = last_segment(class.name.0.as_str());
    let mut out = String::new();

    // Parent + `include`d mixins both become Python base classes,
    // flattened to their last segment like the class name itself.
    let mut bases: Vec<String> = Vec::new();
    if let Some(parent) = &class.parent {
        bases.push(python_base_class(parent.0.as_str()));
    }
    for inc in &class.includes {
        bases.push(last_segment(inc.0.as_str()).to_string());
    }
    if bases.is_empty() {
        writeln!(out, "class {name}:").unwrap();
    } else {
        writeln!(out, "class {name}({}):", bases.join(", ")).unwrap();
    }

    // Accessor reader/writer methods collapse into class-level annotated
    // fields (deduped by attribute name, `notice=` and `notice` sharing
    // one `notice` field). Everything else emits as a method.
    let mut fields: Vec<(String, Ty)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in class.methods.iter().filter(|m| is_accessor(m)) {
        let field = m.name.as_str().trim_end_matches('=').to_string();
        if seen.insert(field.clone()) {
            fields.push((field, accessor_field_ty(m)));
        }
    }
    let methods: Vec<&MethodDef> = class.methods.iter().filter(|m| !is_accessor(m)).collect();

    if fields.is_empty() && methods.is_empty() {
        writeln!(out, "    pass").unwrap();
        return Ok(out);
    }
    for (name, ty) in &fields {
        writeln!(out, "    {name}: {}", python_ty(ty)).unwrap();
    }
    for (i, m) in methods.iter().enumerate() {
        if i > 0 || !fields.is_empty() {
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
        let (mut params, ret_ty) = params_and_ret(m);
        if super::expr::body_contains_yield(&m.body) {
            params.push("_block".to_string());
        }
        writeln!(
            out,
            "def {}({}) -> {}:",
            super::shared::py_method_name(m.name.as_str()),
            params.join(", "),
            python_ty(&ret_ty)
        )
        .unwrap();
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
