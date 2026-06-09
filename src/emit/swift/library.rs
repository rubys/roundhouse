//! `LibraryClass` → Swift file.
//!
//! Renders a lowered class to idiomatic Swift (ported from
//! `src/emit/kotlin/library.rs`):
//!   - `attr_reader`/`attr_writer` accessor methods collapse into Swift
//!     `var` properties (the property *is* the accessor); the synthetic
//!     getter/setter `MethodDef`s are dropped.
//!   - Instance `Method`s → `func`; class methods (`def self.x`) →
//!     `static func` members (no companion-object wrapper — and Swift
//!     statics ARE inherited, unlike Kotlin companions).
//!   - Ruby's implicit return becomes an explicit `return` on the final
//!     statement of value-returning methods.
//!   - Swift requires every parameter typed, so params take their
//!     signature type, falling back to `Any?`. Params are
//!     underscore-labeled (`_ x: T`) — the lowered IR calls
//!     positionally; named-arg call sites are the Phase 5 kwargs story.
//!   - No `open`/`override` modifiers yet: the whole emit is one module
//!     (`open` is cross-module-only in Swift), and `override` marking
//!     needs the class-hierarchy registry — the Phase 2.x cluster.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::emit::EmittedFile;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

use super::expr::{begin_method, emit_expr, wrap_return};
use super::naming::{camel, type_name};
use super::ty::swift_ty;

/// Pre-register every class's instance-method names so zero-arg call
/// sites resolve property-vs-method regardless of render order. Called
/// once by `swift::emit` before the render loop (also resets the
/// per-emit registries).
pub fn register_classes(lcs: &[LibraryClass]) {
    super::expr::reset_registries();
    for lc in lcs {
        let methods: std::collections::HashSet<String> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Instance && m.kind == AccessorKind::Method)
            .map(|m| camel(m.name.as_str()))
            .collect();
        super::expr::register_class_methods(type_name(lc.name.0.as_str()), methods);
    }
}

/// Emit a `LibraryClass` as a standalone Swift file under
/// `Sources/App/app/models/<Name>.swift`.
pub fn emit_class_file(lc: &LibraryClass) -> EmittedFile {
    let class_name = type_name(lc.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("Sources/App/app/models/{class_name}.swift")),
        content: emit_library_class(lc),
    }
}

pub fn emit_library_class(lc: &LibraryClass) -> String {
    let class_name = type_name(lc.name.0.as_str());

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
    //    typed from their assign sites when the lowerer stamped one,
    //    else `Any?`.
    let mut body_ivars: BTreeMap<String, IvarInfo> = BTreeMap::new();
    for m in &lc.methods {
        collect_ivars(&m.body, &mut body_ivars);
    }

    // Install the property-type map so the expression walker can coerce
    // untyped-map → typed-property assigns (`assign_from_row`/`update`).
    super::expr::set_instance_prop_types(
        prop_types.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    );

    let mut out = String::new();

    // Class header + parent.
    let header = match &lc.parent {
        Some(p) => format!("class {class_name}: {}", type_name(p.0.as_str())),
        None => format!("class {class_name}"),
    };
    out.push_str(&header);
    out.push_str(" {\n");

    // Properties.
    for (n, ty) in &prop_types {
        out.push_str(&format!("    var {n}: {} = {}\n", swift_ty(ty), default_for(ty)));
    }
    for (n, info) in &body_ivars {
        if prop_types.contains_key(n) {
            continue;
        }
        match (&info.ty, info.saw_nil) {
            (Some(t), false) => {
                out.push_str(&format!("    var {n}: {} = {}\n", swift_ty(t), default_for(t)));
            }
            (Some(t), true) => {
                let mut st = swift_ty(t);
                if !st.ends_with('?') {
                    st.push('?');
                }
                out.push_str(&format!("    var {n}: {st} = nil\n"));
            }
            (None, _) => {
                out.push_str(&format!("    var {n}: Any? = nil\n"));
            }
        }
    }
    if !prop_types.is_empty() || !body_ivars.is_empty() {
        out.push('\n');
    }

    // Instance methods (skip accessors).
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Instance && m.kind == AccessorKind::Method {
            out.push_str(&indent_method(&emit_method(m, false)));
            out.push('\n');
        }
    }

    // Class methods → `static func` members.
    for m in &lc.methods {
        if m.receiver == MethodReceiver::Class {
            out.push_str(&indent_method(&emit_method(m, true)));
            out.push('\n');
        }
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

fn emit_method(m: &MethodDef, is_static: bool) -> String {
    // Ruby `[]` / `[]=` are emitted as plain `get`/`set` funcs for now;
    // merging the pair into a Swift `subscript` declaration is a Phase 3
    // concern (the runtime's ActiveRecord Base is the only definer).
    let (name, force_unit) = match m.name.as_str() {
        "[]" => ("get".to_string(), false),
        "[]=" => ("set".to_string(), true),
        _ => (camel(m.name.as_str()), false),
    };

    // Params — always typed (Swift requirement), underscore-labeled so
    // the positional lowered call sites work.
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => Some(params),
        _ => None,
    };
    let params: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let pn = camel(p.name.as_str());
            let ty = sig_params
                .and_then(|sp| sp.get(i))
                .map(|sp| swift_ty(&sp.ty))
                .unwrap_or_else(|| "Any?".to_string());
            match &p.default {
                Some(d) => format!("_ {pn}: {ty} = {}", emit_expr(d)),
                None => format!("_ {pn}: {ty}"),
            }
        })
        .collect();

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
            Some(t) if !matches!(t, Ty::Nil) => format!(" -> {}", swift_ty(t)),
            _ => String::new(),
        }
    };

    begin_method(&m.body);
    let body = emit_body(&m.body, returns_value, ret_ty.as_ref());

    let static_kw = if is_static { "static " } else { "" };
    format!(
        "{static_kw}func {name}({}){ret_clause} {{\n{}\n}}\n",
        params.join(", "),
        indent4(&body)
    )
}

fn indent4(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Emit a method body, adding an explicit `return` to the final statement
/// when the method returns a value (Ruby implicit return → Swift). An
/// EMPTY value-returning body (the load-bearing-empty `_adapter_*` /
/// association-stub pattern) synthesizes a default return so the file
/// compiles.
fn emit_body(body: &Expr, returns_value: bool, ret_ty: Option<&Ty>) -> String {
    if !returns_value {
        return emit_expr(body);
    }
    if is_empty_body(body) {
        return default_return(ret_ty);
    }
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => super::expr::emit_stmts(exprs, true),
        _ => wrap_return(body),
    }
}

fn is_empty_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*body.node, ExprNode::Lit { value: crate::expr::Literal::Nil })
}

/// The synthesized statement for an empty value-returning body.
fn default_return(ret_ty: Option<&Ty>) -> String {
    match ret_ty {
        Some(Ty::Int) => "return 0".to_string(),
        Some(Ty::Float) => "return 0.0".to_string(),
        Some(Ty::Bool) => "return false".to_string(),
        Some(Ty::Str) | Some(Ty::Sym) => "return \"\"".to_string(),
        Some(Ty::Array { .. }) => "return []".to_string(),
        Some(Ty::Hash { .. }) => "return [:]".to_string(),
        Some(Ty::Union { variants })
            if variants.iter().any(|v| matches!(v, Ty::Nil)) =>
        {
            "return nil".to_string()
        }
        _ => "fatalError(\"unimplemented\")".to_string(),
    }
}

/// Body-ivar inventory: camelCased name → inferred declaration. The type
/// comes from assign sites — the first concrete `Ty` stamped on an
/// assigned value (the lowerer's hash-field Ty stamping reaches ivar
/// assigns) wins; an ivar that is ever assigned a `nil` literal becomes
/// optional. No signal at all degrades to `Any?` — the same Any?-soup
/// Kotlin started with, escaped the same way.
#[derive(Default, Clone)]
struct IvarInfo {
    ty: Option<Ty>,
    saw_nil: bool,
}

fn collect_ivars(e: &Expr, out: &mut BTreeMap<String, IvarInfo>) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            out.entry(camel(name.as_str())).or_default();
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let info = out.entry(camel(name.as_str())).or_default();
            if matches!(&*value.node, ExprNode::Lit { value: crate::expr::Literal::Nil }) {
                info.saw_nil = true;
            } else if info.ty.is_none() {
                if let Some(t) = value.ty.as_ref() {
                    if !matches!(t, Ty::Nil | Ty::Untyped | Ty::Var { .. }) {
                        info.ty = Some(t.clone());
                    }
                }
            }
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

/// Default initializer for a property type (emitted stored properties
/// are initialized so subclasses keep the inherited memberwise-free
/// default `init`).
fn default_for(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0".to_string(),
        Ty::Float => "0.0".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "\"\"".to_string(),
        Ty::Array { .. } => "[]".to_string(),
        Ty::Hash { .. } => "[:]".to_string(),
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            "nil".to_string()
        }
        _ => "nil".to_string(),
    }
}
