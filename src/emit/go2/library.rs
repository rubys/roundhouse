//! Generic LibraryClass → Go emit (Phase 1 stub).
//!
//! Mirrors `src/emit/rust2/library.rs` but emits Go.
//!
//! Phase 1 scope: produce SYNTACTICALLY valid Go for every method
//! shape — empty body returning the zero value of the declared return
//! type. The goal is to let `runtime_loader::go_units` drive end-to-end
//! and surface compile errors against real method calls, NOT to emit
//! correct semantics. Subsequent sessions land real body emit
//! (expression walker, str_color analog, ownership/copy rules, etc.).
//!
//! Output shape:
//! - Each LibraryClass becomes `type <Name> struct {}` plus one
//!   `func (*<Name>) <method>(args) ret { panic("go2 stub") }` per
//!   method.
//! - Modules (Mode::Module) become a bag of `func <name>(args) ret { panic("go2 stub") }`.
//! - Constants emit as `var <NAME> interface{} = nil` placeholders.
//!
//! Param + return types render via `super::ty::go_ty_stub` — a fully
//! permissive variant that always returns `interface{}` for unknown
//! shapes. The real `go_ty` lives in `src/emit/go/ty.rs` and works
//! from Rails-domain context; the go2 walk doesn't have that context
//! yet.

use crate::dialect::{LibraryClass, MethodDef, MethodReceiver};
use crate::expr::Expr;

use super::ty::go_ty_stub;

pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    let name = sanitize_type_name(class.name.0.as_str());
    let mut out = String::new();
    out.push_str(&format!("type {name} struct{{}}\n\n"));
    for m in &class.methods {
        out.push_str(&emit_method(&name, m));
        out.push('\n');
    }
    Ok(out)
}

/// `ActionController::Base` → `ActionControllerBase`. Go identifiers
/// can't contain `::` (Ruby's namespace separator); strip it so the
/// emitted type at least file-parses.
fn sanitize_type_name(name: &str) -> String {
    name.replace("::", "")
}

/// Ruby method names allow `?`, `!`, `=` suffixes; Go identifiers
/// don't. Map to Go-friendly suffixes so emitted shapes file-parse.
fn sanitize_method_name(name: &str) -> String {
    // Operator-shape method names (`[]`, `[]=`, `<=>`, `==`, `+`,
    // `-`, ...) need to map to Go identifiers. Handle the common
    // ones explicitly; fall back to a `op_<hex>` form for anything
    // else so we never emit an unparseable identifier.
    match name {
        "[]" => return "op_get".to_string(),
        "[]=" => return "op_set".to_string(),
        "<=>" => return "op_cmp".to_string(),
        "==" => return "op_eq".to_string(),
        "!=" => return "op_ne".to_string(),
        "<" => return "op_lt".to_string(),
        "<=" => return "op_le".to_string(),
        ">" => return "op_gt".to_string(),
        ">=" => return "op_ge".to_string(),
        "+" => return "op_add".to_string(),
        "-" => return "op_sub".to_string(),
        "*" => return "op_mul".to_string(),
        "/" => return "op_div".to_string(),
        "%" => return "op_mod".to_string(),
        "<<" => return "op_lshift".to_string(),
        ">>" => return "op_rshift".to_string(),
        "&" => return "op_and".to_string(),
        "|" => return "op_or".to_string(),
        "^" => return "op_xor".to_string(),
        "~" => return "op_inv".to_string(),
        _ => {}
    }
    let mapped = name
        .replace("=", "_eq")
        .replace("?", "_p")
        .replace("!", "_bang");
    if mapped.is_empty() {
        "method".to_string()
    } else {
        mapped
    }
}

pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    let mut out = String::new();
    for m in methods {
        let params = render_params(m);
        let ret = render_return(m);
        let name = sanitize_method_name(m.name.as_str());
        out.push_str(&format!(
            "func {name}({params}){ret} {{\n\tpanic(\"go2 stub: {name}\")\n}}\n\n",
        ));
    }
    Ok(out)
}

pub fn format_constant(name: &str, _value: &Expr) -> String {
    // Phase 1: emit a `var` placeholder. Real const emit needs a Go
    // literal renderer over `Expr`, which is part of the per-target
    // expression walker landing in subsequent sessions.
    format!("var {name} interface{{}} = nil")
}

fn emit_method(class_name: &str, m: &MethodDef) -> String {
    let params = render_params(m);
    let ret = render_return(m);
    let receiver = match m.receiver {
        MethodReceiver::Instance => format!("(self *{class_name}) "),
        MethodReceiver::Class => String::new(),
    };
    let method = sanitize_method_name(m.name.as_str());
    let class_method_name = match m.receiver {
        MethodReceiver::Instance => method.clone(),
        // Class methods emit as bare functions prefixed with the
        // class name (Go has no class-method dispatch). Concrete
        // call sites would reference `Foo_bar(...)`.
        MethodReceiver::Class => format!("{class_name}_{method}"),
    };
    format!(
        "func {receiver}{class_method_name}({params}){ret} {{\n\tpanic(\"go2 stub: {class_name}.{method}\")\n}}\n",
    )
}

fn render_params(m: &MethodDef) -> String {
    // Param doesn't carry a per-param Ty (the function-level
    // `signature: Option<Ty>` does, when present, but Phase 1 doesn't
    // yet decompose it). Emit `interface{}` for every param.
    m.params
        .iter()
        .map(|p| format!("{} {}", sanitize(p.name.as_str()), go_ty_stub(None)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Avoid emitting Go reserved words as parameter names. Adds a `_`
/// suffix to any clash; preserves all others unchanged.
fn sanitize(name: &str) -> String {
    const RESERVED: &[&str] = &[
        "break", "case", "chan", "const", "continue", "default",
        "defer", "else", "fallthrough", "for", "func", "go", "goto",
        "if", "import", "interface", "map", "package", "range",
        "return", "select", "struct", "switch", "type", "var",
    ];
    if RESERVED.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

fn render_return(m: &MethodDef) -> String {
    // Method-level signature in MethodDef.signature is the function
    // type when present; for Phase 1 we conservatively emit
    // `interface{}` so every body's `panic(...)` is type-compatible.
    // A void return needs an empty string (no `func F() {}` trailing
    // space). Use the simple heuristic: signature absent → return
    // `interface{}`; signature present → also `interface{}` (we
    // don't decompose function Tys yet).
    match &m.signature {
        Some(_) => " interface{}".to_string(),
        None => " interface{}".to_string(),
    }
}
