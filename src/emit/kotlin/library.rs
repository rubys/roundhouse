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

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::emit::EmittedFile;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;

use super::expr::{begin_method, emit_expr, set_instance_props, set_return_label};
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
    // A module is an `object` with only functions — no instance props, so
    // every `self.x` send is a method call.
    set_instance_props(HashSet::new());
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
    // Kotlin `object`. Class-level `attr_accessor` (from `class << self`)
    // collapses to an object `var` property; everything else is a `fun`.
    if lc.is_module {
        set_instance_props(HashSet::new());
        let accessor_props = class_accessor_props(&lc.methods);
        let mut out = format!("object {class_name} {{\n");
        for (n, ty) in &accessor_props {
            out.push_str(&format!("    {}\n", object_property_decl(n, ty)));
        }
        if !accessor_props.is_empty() {
            out.push('\n');
        }
        for m in &lc.methods {
            // Skip the synthetic getter/setter funs — the `var` is the
            // accessor.
            if m.kind == AccessorKind::Method {
                out.push_str(&indent_method(&emit_method(m)));
                out.push('\n');
            }
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

    // Parent class. Ruby's StandardError/RuntimeError → Kotlin
    // RuntimeException. A `super(args)` inside `initialize` becomes the
    // supertype constructor call in the header.
    let parent_name = lc.parent.as_ref().map(|p| {
        let last = p.0.as_str().rsplit("::").next().unwrap_or(p.0.as_str());
        match last {
            "StandardError" | "RuntimeError" => "RuntimeException".to_string(),
            other => other.to_string(),
        }
    });
    let super_args = init.and_then(|m| find_super_args(&m.body));
    let parent_clause = match (&parent_name, &super_args) {
        (Some(pn), Some(args)) => format!(" : {pn}({})", args.join(", ")),
        (Some(pn), None) => format!(" : {pn}()"),
        (None, _) => String::new(),
    };
    let header = match init {
        Some(m) => format!("class {class_name}({}){parent_clause}", method_params(m).join(", ")),
        None => format!("class {class_name}{parent_clause}"),
    };
    out.push_str(&header);
    out.push_str(" {\n");

    // Properties. Constructor-param-backed properties are assigned in the
    // `init` block, so they need no initializer (and a non-null type like
    // `Base` can't be defaulted to null anyway).
    let ctor_param_names: std::collections::HashSet<String> = init
        .map(|m| m.params.iter().map(|p| camel(p.name.as_str())).collect())
        .unwrap_or_default();
    for (n, ty) in &prop_types {
        if ctor_param_names.contains(n) {
            out.push_str(&format!("    var {n}: {}\n", kotlin_ty(ty)));
        } else {
            out.push_str(&format!("    var {n}: {} = {}\n", kotlin_ty(ty), default_for(ty)));
        }
    }
    let inferred_ivar_types = infer_body_ivar_types(&lc.methods);
    for n in body_ivars.keys() {
        if !prop_types.contains_key(n) {
            match inferred_ivar_types.get(n) {
                Some(ty) => out.push_str(&format!(
                    "    var {n}: {} = {}\n",
                    kotlin_ty(ty),
                    default_for(ty)
                )),
                None => out.push_str(&format!("    var {n}: Any? = null\n")),
            }
        }
    }
    if !prop_types.is_empty() || !body_ivars.is_empty() {
        out.push('\n');
    }

    // The class's property names (accessor-backed + body ivars), so a
    // `self.x` zero-arg send in a body emits as a property read; everything
    // else gets `()`. Active for the rest of this function's method emit.
    let instance_props: HashSet<String> =
        prop_types.keys().chain(body_ivars.keys()).cloned().collect();
    set_instance_props(instance_props);

    // init block (initialize body). Kotlin `init` can't `return`, so when
    // the body has a guard `return`, wrap it in `run { }` and emit
    // `return@run`.
    if let Some(m) = init {
        begin_method(&m.body);
        let has_return = body_has_return(&m.body);
        if has_return {
            set_return_label(Some("run"));
        }
        let body = emit_body(&m.body, false);
        set_return_label(None);
        let inner = if has_return {
            format!("run {{\n{}\n}}", indent4(&body))
        } else {
            body
        };
        out.push_str(&format!("    init {{\n{}\n    }}\n\n", indent4(&indent4(&inner))));
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

    let mut params = method_params(m);
    // A method that `yield`s takes a `block` parameter in Kotlin (there's
    // no implicit block); `yield` calls it. Type from the signature's
    // block slot.
    if body_has_yield(&m.body) {
        let bt = match m.signature.as_ref() {
            Some(Ty::Fn { block: Some(b), .. }) => kotlin_ty(b),
            _ => "(Any?) -> Unit".to_string(),
        };
        params.push(format!("block: {bt}"));
    }

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
    // A value-returning method with an empty body (Ruby `def x; end` →
    // implicit `nil`) can't emit a bare `return` in Kotlin — a non-Unit
    // function must yield a value. These are the load-bearing-empty AR
    // overrides (`_adapter_insert`, `_adapter_reload`, …); subclasses
    // override, so the base body never runs. Synthesize the type's default
    // (`0` for `Long`, `null` for nullable returns) to keep it a no-op.
    let body = if returns_value && is_empty_body(&m.body) {
        let ret = ret_ty.clone().unwrap_or(Ty::Untyped);
        format!("return {}", default_for(&ret))
    } else {
        emit_body(&m.body, returns_value)
    };

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
    // A `raise Class, msg` send emits as a `throw` (type `Nothing`); like a
    // `Raise` node it needs no `return` prefix.
    let is_raise_send = matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, .. } if method.as_str() == "raise"
    );
    let no_return = is_raise_send
        || matches!(
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

fn body_has_yield(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Yield { .. }) || expr_children(e).iter().any(|c| body_has_yield(c))
}

/// Find a `super(args)` call (delegated to the parent constructor in the
/// class header). Returns the emitted arg strings, or `None` if there's
/// no `super` (or it's `super()` with no args returns `Some(vec![])`).
fn find_super_args(e: &Expr) -> Option<Vec<String>> {
    if let ExprNode::Super { args } = &*e.node {
        return Some(
            args.as_ref()
                .map(|a| a.iter().map(emit_expr).collect())
                .unwrap_or_default(),
        );
    }
    for c in expr_children(e) {
        if let Some(r) = find_super_args(c) {
            return Some(r);
        }
    }
    None
}

fn body_has_return(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Return { .. })
        || expr_children(e).iter().any(|c| body_has_return(c))
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

/// Pre-scan hook (runs before rendering, via the `kotlin_units` transform):
/// register every module/object-level accessor property so a
/// `Const`-receiver read of it (`ActiveRecord.adapter`) drops its parens.
/// Order-independent — the registry is populated for all classes in an
/// entry before any of them render.
pub fn register_object_accessors(classes: &[LibraryClass]) {
    for lc in classes {
        if !lc.is_module {
            continue;
        }
        let object = lc.name.0.as_str().rsplit("::").next().unwrap_or(lc.name.0.as_str());
        for prop in class_accessor_props(&lc.methods).keys() {
            super::expr::register_object_accessor(object, prop);
        }
    }
}

/// Collect class-level (`receiver == Class`) accessor properties — the
/// `class << self; attr_accessor :x` pairs — as camelCased name → type
/// (from the reader's RBS return / the writer's param). Instance accessors
/// are handled separately (they collapse to instance `var`s).
fn class_accessor_props(methods: &[MethodDef]) -> BTreeMap<String, Ty> {
    let mut props: BTreeMap<String, Ty> = BTreeMap::new();
    for m in methods {
        if m.receiver != MethodReceiver::Class {
            continue;
        }
        match m.kind {
            AccessorKind::AttributeReader => {
                if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
                    props.entry(camel(m.name.as_str())).or_insert_with(|| (**ret).clone());
                }
            }
            AccessorKind::AttributeWriter => {
                if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
                    if let Some(p) = params.first() {
                        let base = m.name.as_str().trim_end_matches('=');
                        props.entry(camel(base)).or_insert_with(|| p.ty.clone());
                    }
                }
            }
            AccessorKind::Method => {}
        }
    }
    props
}

/// Declaration for an object-level accessor property. A non-null reference
/// type (the global adapter slot) is `lateinit var` — set once at boot, so
/// a nullable default would force `!!` at every read; primitives/nullables
/// fall back to a defaulted `var`.
fn object_property_decl(name: &str, ty: &Ty) -> String {
    let kt = kotlin_ty(ty);
    if can_lateinit(ty) {
        format!("lateinit var {name}: {kt}")
    } else {
        format!("var {name}: {kt} = {}", default_for(ty))
    }
}

/// `lateinit` is legal only for non-null, non-primitive types.
fn can_lateinit(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Nil | Ty::Untyped | Ty::Var { .. } => false,
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => false,
        _ => true,
    }
}

/// True when a method body is empty (`def x; end` → an empty `Seq`).
fn is_empty_body(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
}

/// Infer types for body-only ivars (those without an `attr_*` accessor) so
/// they don't all collapse to `Any?`. Two signals, strongest first:
///   1. A pure reader method whose body *is* the ivar (`def errors;
///      @errors; end`) donates its declared return type — this is how
///      `@errors`/`@persisted`/`@destroyed` get `Array[String]`/`bool`
///      from `base.rbs` without an ivar-declaration syntax in RBS.
///   2. Otherwise, the literal an ivar is assigned (`@persisted = false`).
/// Same shape works for flash/session's `@data` once they're wired.
fn infer_body_ivar_types(methods: &[MethodDef]) -> BTreeMap<String, Ty> {
    let mut out: BTreeMap<String, Ty> = BTreeMap::new();

    // Signal 1: reader methods returning exactly an ivar.
    for m in methods {
        if let (Some(ivar), Some(Ty::Fn { ret, .. })) =
            (body_returns_ivar(&m.body), m.signature.as_ref())
        {
            if !matches!(&**ret, Ty::Nil) {
                out.entry(ivar).or_insert_with(|| (**ret).clone());
            }
        }
    }

    // Signal 2: literal assignments, only for ivars not already inferred.
    for m in methods {
        collect_ivar_literal_types(&m.body, &mut out);
    }

    out
}

/// If a method body is (or ends in) a bare ivar read, return that ivar's
/// camel-cased name. Covers `@x`, `return @x`, and a `Seq` ending in `@x`.
fn body_returns_ivar(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Ivar { name } => Some(camel(name.as_str())),
        ExprNode::Return { value } => body_returns_ivar(value),
        ExprNode::Seq { exprs } => exprs.last().and_then(body_returns_ivar),
        _ => None,
    }
}

/// Record `@ivar = <literal>` types as a fallback, never overwriting a
/// type already established by a reader (signal 1 is stronger).
fn collect_ivar_literal_types(e: &Expr, out: &mut BTreeMap<String, Ty>) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*e.node {
        if let Some(ty) = literal_ty(value) {
            out.entry(camel(name.as_str())).or_insert(ty);
        }
    }
    for child in expr_children(e) {
        collect_ivar_literal_types(child, out);
    }
}

/// The `Ty` of a literal expression, when it's unambiguous from the node.
fn literal_ty(e: &Expr) -> Option<Ty> {
    use crate::expr::Literal;
    match &*e.node {
        ExprNode::Lit { value: Literal::Bool { .. } } => Some(Ty::Bool),
        ExprNode::Lit { value: Literal::Int { .. } } => Some(Ty::Int),
        ExprNode::Lit { value: Literal::Float { .. } } => Some(Ty::Float),
        ExprNode::Lit { value: Literal::Str { .. } } => Some(Ty::Str),
        _ => None,
    }
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
