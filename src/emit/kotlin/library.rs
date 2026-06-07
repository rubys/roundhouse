//! `LibraryClass` → Kotlin file.
//!
//! Renders a lowered class to idiomatic Kotlin:
//!   - `attr_reader`/`attr_writer` accessor methods collapse into Kotlin
//!     `var` properties (the property *is* the accessor); the synthetic
//!     getter/setter `MethodDef`s are dropped.
//!   - Instance `Method`s → `fun`; class methods (`def self.x`) →
//!     `companion object` members.
//!   - Ruby's implicit return becomes an explicit `return` on the final
//!     statement of value-returning methods (Kotlin block bodies don't
//!     implicitly return).
//!   - Kotlin requires every parameter typed, so params take their
//!     signature type, falling back to `Any?`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::emit::EmittedFile;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

use super::expr::{begin_method, emit_expr};
use super::naming::camel;
use super::ty::kotlin_ty;

/// Emit a `LibraryClass` as a standalone Kotlin file under
/// `src/main/kotlin/app/models/<Name>.kt`.
pub fn emit_class_file(lc: &LibraryClass) -> EmittedFile {
    let name = lc.name.0.as_str();
    let last = name.rsplit("::").next().unwrap_or(name);
    EmittedFile {
        path: PathBuf::from(format!("src/main/kotlin/app/models/{last}.kt")),
        content: format!("package roundhouse\n\n{}", emit_library_class(lc)),
    }
}

/// `Result`-returning wrapper for the `runtime_loader::TargetEmit` slot.
pub fn emit_library_class_result(lc: &LibraryClass) -> Result<String, String> {
    Ok(emit_library_class(lc))
}

/// Render a Ruby `module X` (parsed as a set of class methods) as a
/// Kotlin `object X { ... }`. Used for `Mode::Module` runtime entries
/// (e.g. `inflector.rb`). The module name comes from the methods'
/// `enclosing_class`.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    let name = methods
        .first()
        .and_then(|m| m.enclosing_class.as_ref())
        .map(|s| s.as_str().rsplit("::").next().unwrap_or(s.as_str()).to_string())
        .unwrap_or_default();
    let mut out = format!("object {name} {{\n");
    for m in methods {
        out.push_str(&indent_method(&emit_method(m)));
        out.push('\n');
    }
    out.push_str("}\n");
    Ok(out)
}

pub fn emit_library_class(lc: &LibraryClass) -> String {
    let name = lc.name.0.as_str();
    let class_name = name.rsplit("::").next().unwrap_or(name).to_string();

    // A Ruby `module` (only module-functions, no instance state) → a
    // Kotlin `object`. All methods render as plain `fun`.
    if lc.is_module {
        let mut out = format!("object {class_name} {{\n");
        for m in &lc.methods {
            out.push_str(&indent_method(&emit_method(m)));
            out.push('\n');
        }
        out.push_str("}\n");
        return out;
    }

    // 1. Accessor-derived properties (name → type), and the set of method
    //    names to drop (the synthesized getters/setters).
    let mut prop_types: BTreeMap<String, Ty> = BTreeMap::new();
    for m in &lc.methods {
        match m.kind {
            AccessorKind::AttributeReader => {
                if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
                    prop_types
                        .entry(camel(m.name.as_str()))
                        .or_insert_with(|| (**ret).clone());
                }
            }
            AccessorKind::AttributeWriter => {
                if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
                    if let Some(p) = params.first() {
                        // writer name is `foo=`; strip the `=`.
                        let base = m.name.as_str().trim_end_matches('=');
                        prop_types.entry(camel(base)).or_insert_with(|| p.ty.clone());
                    }
                }
            }
            AccessorKind::Method => {}
        }
    }

    // 2. Body-only ivars (e.g. `@comments_cache`) that have no accessor —
    //    declared as `Any?` since we have no signature for them.
    let mut body_ivars: BTreeMap<String, ()> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }

    let mut out = String::new();

    // Ruby `initialize` → Kotlin primary constructor + `init` block. The
    // constructor params shadow the same-named properties inside `init`,
    // where ivar writes are `this.`-qualified.
    let init = lc
        .methods
        .iter()
        .find(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == "initialize");

    let parent_clause = match &lc.parent {
        Some(p) => {
            let pn = p.0.as_str().rsplit("::").next().unwrap_or(p.0.as_str());
            format!(" : {pn}()")
        }
        None => String::new(),
    };
    let header = match init {
        Some(m) => format!("class {class_name}({}){parent_clause}", method_params(m).join(", ")),
        None => format!("class {class_name}{parent_clause}"),
    };
    out.push_str(&header);
    out.push_str(" {\n");

    // Properties.
    for (n, ty) in &prop_types {
        out.push_str(&format!("    var {n}: {} = {}\n", kotlin_ty(ty), default_for(ty)));
    }
    for n in body_ivars.keys() {
        if !prop_types.contains_key(n) {
            out.push_str(&format!("    var {n}: Any? = null\n"));
        }
    }
    if !prop_types.is_empty() || !body_ivars.is_empty() {
        out.push('\n');
    }

    // init block (initialize body, no return wrapping).
    if let Some(m) = init {
        begin_method(&m.body);
        let body = emit_body(&m.body, false);
        out.push_str(&format!("    init {{\n{}\n    }}\n\n", indent4(&indent4(&body))));
    }

    // Instance methods (skip accessors and the initialize we just used).
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Instance
            && m.kind == AccessorKind::Method
            && m.name.as_str() != "initialize"
        {
            out.push_str(&indent_method(&emit_method(m)));
            out.push('\n');
        }
    }

    // Class methods → companion object.
    let class_methods: Vec<&MethodDef> = lc
        .methods
        .iter()
        .filter(|m| m.receiver == MethodReceiver::Class)
        .collect();
    if !class_methods.is_empty() {
        out.push_str("    companion object {\n");
        for m in class_methods {
            out.push_str(&indent_method(&indent_method(&emit_method(m))));
            out.push('\n');
        }
        out.push_str("    }\n");
    }

    out.push_str("}\n");
    out
}

fn indent_method(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_method(m: &MethodDef) -> String {
    // Ruby `[]` / `[]=` → Kotlin indexing operators. `set` is always
    // Unit-returning (the source RBS union return is dropped).
    let (decl_kw, name, force_unit) = match m.name.as_str() {
        "[]" => ("operator fun", "get".to_string(), false),
        "[]=" => ("operator fun", "set".to_string(), true),
        _ => ("fun", camel(m.name.as_str()), false),
    };

    let params = method_params(m);

    // Return type.
    let ret_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let returns_value = !force_unit && matches!(&ret_ty, Some(t) if !matches!(t, Ty::Nil));
    let ret_clause = if force_unit {
        String::new()
    } else {
        match &ret_ty {
            Some(t) if !matches!(t, Ty::Nil) => format!(": {}", kotlin_ty(t)),
            _ => String::new(),
        }
    };

    begin_method(&m.body);
    let body = emit_body(&m.body, returns_value);

    format!("{decl_kw} {name}({}){ret_clause} {{\n{}\n}}\n", params.join(", "), indent4(&body))
}

/// Render a method's params, always typed (Kotlin requirement); falls
/// back to `Any?` where the signature is missing.
fn method_params(m: &MethodDef) -> Vec<String> {
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => Some(params),
        _ => None,
    };
    m.params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let pn = camel(p.name.as_str());
            let ty = sig_params
                .and_then(|sp| sp.get(i))
                .map(|sp| kotlin_ty(&sp.ty))
                .unwrap_or_else(|| "Any?".to_string());
            match &p.default {
                Some(d) => format!("{pn}: {ty} = {}", emit_expr(d)),
                None => format!("{pn}: {ty}"),
            }
        })
        .collect()
}

fn indent4(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Emit a method body, adding an explicit `return` to the final statement
/// when the method returns a value (Ruby implicit return → Kotlin).
fn emit_body(body: &Expr, returns_value: bool) -> String {
    if !returns_value {
        return emit_expr(body);
    }
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = exprs[..exprs.len() - 1].iter().map(emit_expr).collect();
            lines.push(wrap_return(&exprs[exprs.len() - 1]));
            lines.join("\n")
        }
        _ => wrap_return(body),
    }
}

/// Prefix `return` unless the expression is already terminal or is a
/// statement that has no value (assignment, loop).
fn wrap_return(e: &Expr) -> String {
    let s = emit_expr(e);
    let no_return = matches!(
        &*e.node,
        ExprNode::Return { .. }
            | ExprNode::Raise { .. }
            | ExprNode::While { .. }
            | ExprNode::Assign { .. }
            | ExprNode::Super { .. }
            | ExprNode::Next { .. }
            | ExprNode::Break { .. }
    );
    if no_return {
        s
    } else {
        format!("return {s}")
    }
}

fn collect_ivars(e: &Expr, out: &mut BTreeMap<String, ()>) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            out.insert(camel(name.as_str()), ());
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            out.insert(camel(name.as_str()), ());
            collect_ivars(value, out);
        }
        _ => {}
    }
    for child in expr_children(e) {
        collect_ivars(child, out);
    }
}

fn expr_children(e: &Expr) -> Vec<&Expr> {
    let mut v = Vec::new();
    match &*e.node {
        ExprNode::Seq { exprs } => v.extend(exprs.iter()),
        ExprNode::If { cond, then_branch, else_branch } => {
            v.push(cond);
            v.push(then_branch);
            v.push(else_branch);
        }
        ExprNode::While { cond, body, .. } => {
            v.push(cond);
            v.push(body);
        }
        ExprNode::Assign { value, .. } => v.push(value),
        ExprNode::Case { scrutinee, arms } => {
            v.push(scrutinee);
            for a in arms {
                v.push(&a.body);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                v.push(r);
            }
            v.extend(args.iter());
            if let Some(b) = block {
                v.push(b);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            v.push(left);
            v.push(right);
        }
        ExprNode::Return { value } | ExprNode::Raise { value } => v.push(value),
        ExprNode::Lambda { body, .. } => v.push(body),
        ExprNode::Hash { entries, .. } => {
            for (k, val) in entries {
                v.push(k);
                v.push(val);
            }
        }
        ExprNode::Array { elements, .. } => v.extend(elements.iter()),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    v.push(expr);
                }
            }
        }
        _ => {}
    }
    v
}

/// Default initializer for a property type (Kotlin requires properties
/// be initialized).
fn default_for(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Array { .. } => "mutableListOf()".to_string(),
        Ty::Hash { .. } => "mutableMapOf()".to_string(),
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            "null".to_string()
        }
        _ => "null".to_string(),
    }
}
