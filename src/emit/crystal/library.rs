//! Library-shape Crystal emission — the `LibraryClass` walker.
//!
//! Mirrors `src/emit/ruby/library.rs` (Spinel) with three Crystal
//! divergences:
//!   - File extension `.cr` instead of `.rb`.
//!   - `require "./relative_path"` instead of Ruby's `require_relative`.
//!     The path-resolution logic is identical; only the keyword differs.
//!   - Methods carry type-annotated signatures (rendered by
//!     `super::method::emit_method`), and ivar declarations land at the
//!     class header so Crystal's strict typing accepts them.
//!
//! Output mirrors Spinel's directory layout — one file per
//! `LibraryClass` under `src/<dir>/<stem>.cr` (e.g. `src/models/article.cr`,
//! `src/views/articles/index.cr`). Module/class headers nest naturally
//! when the class name carries `::` segments (`Views::Articles`).

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use super::super::EmittedFile;
use super::method::emit_method as emit_method_impl;
use crate::App;
use crate::dialect::{LibraryClass, LibraryFunction, MethodDef};
use crate::expr::{Expr, ExprNode, InterpPart};
use crate::ident::ClassId;
use crate::naming::snake_case;

/// Emit a synthesized `LibraryClass{is_module:true}` from a list of
/// `LibraryFunction`s sharing a `module_path`. Mirrors Spinel's
/// `emit_module_file`.
pub fn emit_module_file(
    funcs: &[LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    if funcs.is_empty() {
        return EmittedFile { path: out_path, content: String::new() };
    }
    let lc = synthesize_module_lc(funcs);
    emit_library_class_decl(&lc, app, out_path)
}

fn synthesize_module_lc(funcs: &[LibraryFunction]) -> LibraryClass {
    use crate::dialect::{AccessorKind, MethodReceiver};
    use crate::ident::Symbol;

    let module_id = funcs
        .first()
        .map(|f| {
            ClassId(Symbol::from(
                f.module_path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("::"),
            ))
        })
        .unwrap_or_else(|| ClassId(Symbol::from("")));
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            params: f.params.clone(),
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(module_id.0.clone()),
            kind: AccessorKind::Method,
        })
        .collect();
    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
        origin: None,
    }
}

/// Public emit entry — for Module mode (flat list of class methods).
/// Used by `runtime_loader::crystal_units` for `Module`-mode runtime
/// files (e.g. `inflector.rb`).
///
/// Crystal requires explicit `module X ... end` wrapping for class
/// methods to be addressable as `X.method` (unlike TS where bare
/// `export function` + `import * as X` produces the namespace from
/// the import side). The module name is derived from the methods'
/// `enclosing_class` field; missing or inconsistent values trip an
/// error rather than emitting top-level functions that would attach
/// to the implicit Object class.
pub fn emit_module(methods: &[MethodDef]) -> Result<String, String> {
    use crate::dialect::MethodReceiver;
    if methods.is_empty() {
        return Ok(String::new());
    }
    if !methods.iter().all(|m| matches!(m.receiver, MethodReceiver::Class)) {
        return Err(format!(
            "crystal::emit_module: only all-class-method modules supported; \
             saw mixed/instance methods (first instance: `{}`)",
            methods
                .iter()
                .find(|m| matches!(m.receiver, MethodReceiver::Instance))
                .map(|m| m.name.as_str())
                .unwrap_or("<none>"),
        ));
    }

    let module_name = methods
        .first()
        .and_then(|m| m.enclosing_class.as_ref())
        .map(|sym| sym.as_str().to_string())
        .ok_or_else(|| {
            "crystal::emit_module: methods missing `enclosing_class`; \
             cannot synthesize Crystal `module X ... end` wrapping"
                .to_string()
        })?;

    // Compound names like `ActiveRecord::Errors` nest as
    // `module ActiveRecord\n  module Errors`. Same logic as
    // `render_class` for class-shape inputs.
    let segments: Vec<&str> = module_name.split("::").collect();
    let depth = segments.len();
    let body_pad = "  ".repeat(depth);

    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        writeln!(out, "{}module {seg}", "  ".repeat(i)).unwrap();
    }

    let mut first = true;
    for m in methods {
        if !first {
            writeln!(out).unwrap();
        }
        first = false;
        let body = emit_method_impl(m);
        for line in body.lines() {
            if line.is_empty() {
                writeln!(out).unwrap();
            } else {
                writeln!(out, "{body_pad}{line}").unwrap();
            }
        }
    }

    for i in (0..depth).rev() {
        writeln!(out, "{}end", "  ".repeat(i)).unwrap();
    }
    Ok(out)
}

/// Public emit entry — for Library mode (one or more classes per file).
/// Used by `runtime_loader::crystal_units` for `Library`-mode runtime
/// files. Returns a single class declaration; the loader concatenates
/// multiple classes when the source file holds multiple definitions.
pub fn emit_library_class(class: &LibraryClass) -> Result<String, String> {
    Ok(render_class(class))
}

pub(super) fn emit_library_class_decl(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    emit_library_class_decl_with_synthesized(lc, app, out_path, &[])
}

pub(super) fn emit_library_class_decl_with_synthesized(
    lc: &LibraryClass,
    _app: &App,
    out_path: PathBuf,
    _synthesized_siblings: &[(String, String)],
) -> EmittedFile {
    // Crystal's compiler does whole-program analysis, so individual
    // emitted files don't need per-file `require` headers — the
    // aggregator at `src/app.cr` requires every emitted file once,
    // and forward references resolve naturally during type inference.
    // (Spinel emits `require_relative` headers because Ruby has no
    // load-time analysis pass; Crystal removes that constraint.)
    EmittedFile {
        path: out_path,
        content: render_class(lc),
    }
}

/// Render the `module ... end` / `class ... end` text for a single
/// LibraryClass. Crystal-specific transforms applied here:
///
/// * Attribute reader/writer pairs (recognized via
///   `MethodDef.kind: AccessorKind::Attribute*`) collapse to
///   `property NAME : T?` — Crystal's macro that synthesizes the
///   getter, setter, and (when used in `initialize`) keyword-arg
///   defaults. Nilable so default-nil works without explicit init.
/// * Ivar assignments inside method bodies (`@x = …` patterns)
///   become `@x : T?` declarations at the class header so Crystal's
///   strict typing accepts the writes.
/// * The lowered `def initialize(attrs = {})` is skipped — Crystal's
///   compiler auto-synthesizes a keyword-arg initializer from the
///   `property` declarations, which is what callers expect.
///
/// Used by `emit_library_class_decl` (with require headers) and by
/// `emit_library_class` (no headers — the caller supplies them).
fn render_class(lc: &LibraryClass) -> String {
    use crate::dialect::AccessorKind;

    let mut s = String::new();
    let name = lc.name.0.as_str();
    let segments: Vec<&str> = name.split("::").collect();
    let depth = segments.len();
    let body_pad = "  ".repeat(depth);

    if lc.is_module {
        for (i, seg) in segments.iter().enumerate() {
            writeln!(s, "{}module {seg}", "  ".repeat(i)).unwrap();
        }
    } else {
        for (i, seg) in segments.iter().take(depth - 1).enumerate() {
            writeln!(s, "{}module {seg}", "  ".repeat(i)).unwrap();
        }
        let last = segments[depth - 1];
        let pad = "  ".repeat(depth - 1);
        match lc.parent.as_ref() {
            Some(p) => writeln!(s, "{pad}class {last} < {}", crystal_parent_name(p.0.as_str())).unwrap(),
            None => writeln!(s, "{pad}class {last}").unwrap(),
        }
    }

    for inc in &lc.includes {
        writeln!(s, "{body_pad}include {}", inc.0.as_str()).unwrap();
    }

    // Collect attribute properties from attr_reader pairs.
    let mut properties: Vec<(String, String)> = Vec::new();
    let mut accessor_method_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for m in &lc.methods {
        match m.kind {
            AccessorKind::AttributeReader => {
                let ty = match m.signature.as_ref() {
                    Some(crate::ty::Ty::Fn { ret, .. }) => super::ty::crystal_ty(ret),
                    _ => "String".to_string(),
                };
                // `T?` shorthand so the property can default to nil
                // and the auto-synthesized initializer doesn't require
                // the caller to pass it.
                let ty_nilable = if ty.ends_with('?') || ty == "Nil" {
                    ty
                } else {
                    format!("{ty}?")
                };
                properties.push((m.name.as_str().to_string(), ty_nilable));
                accessor_method_names.insert(m.name.as_str().to_string());
            }
            AccessorKind::AttributeWriter => {
                // Setter `def name=(v)` collapses with its reader; if
                // the reader was registered above, the property covers
                // both. Drop the explicit method.
                let base = m.name.as_str().trim_end_matches('=').to_string();
                accessor_method_names.insert(format!("{base}="));
            }
            _ => {}
        }
    }

    // Collect untyped ivar assignments from method bodies for `@x : T?`
    // declarations. Skips ivars already declared as properties.
    let mut ivars: indexmap::IndexMap<String, crate::ty::Ty> = indexmap::IndexMap::new();
    for m in &lc.methods {
        collect_ivar_assignments(&m.body, &mut ivars);
    }
    ivars.retain(|name, _| !properties.iter().any(|(p, _)| p == name));

    // Class header emit: `property` declarations first, then `@ivar`
    // declarations, then methods (with attr_reader/writer methods
    // skipped via `accessor_method_names`).
    let mut wrote_header_lines = false;
    for (name, ty) in &properties {
        writeln!(s, "{body_pad}property {name} : {ty}").unwrap();
        wrote_header_lines = true;
    }
    for (name, ty) in &ivars {
        let ty_s = super::ty::crystal_ty(ty);
        let ty_nilable = if ty_s.ends_with('?') || ty_s == "Nil" {
            ty_s
        } else {
            format!("{ty_s}?")
        };
        writeln!(s, "{body_pad}@{name} : {ty_nilable}").unwrap();
        wrote_header_lines = true;
    }
    if wrote_header_lines && lc.methods.iter().any(|m| !is_skipped_method(m, &accessor_method_names)) {
        writeln!(s).unwrap();
    }

    let mut first = true;
    for m in &lc.methods {
        if is_skipped_method(m, &accessor_method_names) {
            continue;
        }
        if !first {
            writeln!(s).unwrap();
        }
        first = false;
        let body = emit_method_impl(m);
        for line in body.lines() {
            if line.is_empty() {
                writeln!(s).unwrap();
            } else {
                writeln!(s, "{body_pad}{line}").unwrap();
            }
        }
    }

    for i in (0..depth).rev() {
        writeln!(s, "{}end", "  ".repeat(i)).unwrap();
    }
    s
}

/// Skip emit for attr_reader/writer methods that were collapsed into
/// `property` declarations, AND for the lowered
/// `def initialize(attrs = {})` (Crystal auto-synthesizes a keyword-arg
/// initializer from the property declarations — overriding it with a
/// hash-arg version reintroduces the impedance mismatch we just removed).
fn is_skipped_method(
    m: &MethodDef,
    accessor_names: &std::collections::HashSet<String>,
) -> bool {
    if accessor_names.contains(m.name.as_str()) {
        return true;
    }
    if m.name.as_str() == "initialize" && m.params.len() == 1 {
        // Heuristic: the lowerer-emitted shape takes a single `attrs`
        // hash. User-defined initializers with explicit typed params
        // would have multiple params (or a different param name) and
        // would still emit through the normal path.
        let only_param = &m.params[0];
        if only_param.as_str() == "attrs" {
            return true;
        }
    }
    false
}

/// Walk an Expr collecting `@ivar = value` assignments, keyed by ivar
/// name. Type comes from the RHS's analyzer-inferred type, falling back
/// to `Untyped` when no inference happened. Mirror of
/// `typescript::collect_ivar_assignments`.
fn collect_ivar_assignments(
    e: &Expr,
    out: &mut indexmap::IndexMap<String, crate::ty::Ty>,
) {
    use crate::expr::LValue;
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let ty = value.ty.clone().unwrap_or(crate::ty::Ty::Untyped);
            out.insert(name.as_str().to_string(), ty);
            collect_ivar_assignments(value, out);
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                collect_ivar_assignments(recv, out);
            }
            collect_ivar_assignments(value, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivar_assignments(r, out);
            }
            for a in args {
                collect_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                collect_ivar_assignments(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            collect_ivar_assignments(fun, out);
            for a in args {
                collect_ivar_assignments(a, out);
            }
            if let Some(b) = block {
                collect_ivar_assignments(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivar_assignments(k, out);
                collect_ivar_assignments(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                collect_ivar_assignments(el, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    collect_ivar_assignments(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_ivar_assignments(left, out);
            collect_ivar_assignments(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_ivar_assignments(value, out);
            collect_ivar_assignments(body, out);
        }
        ExprNode::Lambda { body, .. } => collect_ivar_assignments(body, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivar_assignments(cond, out);
            collect_ivar_assignments(then_branch, out);
            collect_ivar_assignments(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_ivar_assignments(scrutinee, out);
            for arm in arms {
                collect_ivar_assignments(&arm.body, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_ivar_assignments(e, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_ivar_assignments(a, out);
            }
        }
        ExprNode::Raise { value } => collect_ivar_assignments(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            collect_ivar_assignments(expr, out);
            collect_ivar_assignments(fallback, out);
        }
        ExprNode::Return { value } => collect_ivar_assignments(value, out),
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_ivar_assignments(body, out);
            for r in rescues {
                collect_ivar_assignments(&r.body, out);
            }
            if let Some(e) = else_branch {
                collect_ivar_assignments(e, out);
            }
            if let Some(e) = ensure {
                collect_ivar_assignments(e, out);
            }
        }
        ExprNode::While { cond, body, .. } => {
            collect_ivar_assignments(cond, out);
            collect_ivar_assignments(body, out);
        }
        _ => {}
    }
}

/// Project-root-anchored require target for a parent class. Crystal's
/// runtime files live under `src/` (not `runtime/`); the transpiled
/// framework runtime emits to `src/active_record_base.cr` etc. so
/// parent references resolve there.
fn require_path_for_parent(parent: &ClassId, app: &App) -> Option<String> {
    let raw = parent.0.as_str();
    if raw == "ActiveRecord::Base" {
        return Some("src/active_record_base".to_string());
    }
    if raw == "ActionController::Base" || raw == "ActionController::API" {
        return Some("src/action_controller_base".to_string());
    }
    if app.models.iter().any(|m| m.name.0.as_str() == raw)
        || app.library_classes.iter().any(|lc| lc.name.0.as_str() == raw)
    {
        return Some(format!("src/models/{}", snake_case(raw)));
    }
    if app.controllers.iter().any(|c| c.name.0.as_str() == raw) {
        return Some(format!("src/controllers/{}", snake_case(raw)));
    }
    None
}

fn require_path_for_body_const(
    path: &[String],
    app: &App,
    self_name: &str,
) -> Option<String> {
    let first = path.first()?;
    if first == self_name {
        return None;
    }
    if app.models.iter().any(|m| m.name.0.as_str() == first.as_str())
        || app
            .library_classes
            .iter()
            .any(|lc| lc.name.0.as_str() == first.as_str())
    {
        return Some(format!("src/models/{}", snake_case(first)));
    }
    if app.controllers.iter().any(|c| c.name.0.as_str() == first.as_str()) {
        return Some(format!("src/controllers/{}", snake_case(first)));
    }
    match first.as_str() {
        "Views" => Some("src/views".to_string()),
        "Inflector" => Some("src/inflector".to_string()),
        "ViewHelpers" => Some("src/view_helpers".to_string()),
        "RouteHelpers" => Some("src/route_helpers".to_string()),
        "Importmap" => Some("src/importmap".to_string()),
        "Schema" => Some("src/schema".to_string()),
        "Routes" => Some("src/routes".to_string()),
        "Parameters" => Some("src/parameters".to_string()),
        "Router" => Some("src/router".to_string()),
        _ => None,
    }
}

fn is_same_dir(from_dir: &Path, to_anchor: &str) -> bool {
    let to_dir: String = to_anchor
        .rsplit_once('/')
        .map(|(d, _)| d.to_string())
        .unwrap_or_default();
    from_dir.to_str().unwrap_or("") == to_dir
}

/// Translate a Ruby parent class name to its Crystal equivalent.
/// Crystal's exception hierarchy roots at `Exception` (no
/// `StandardError`); mapping the common Ruby base names keeps
/// transpiled framework runtime files compilable. Unknown names pass
/// through unchanged so app-level inheritance (e.g. `class Article <
/// ApplicationRecord`) still works.
pub(super) fn crystal_parent_name(ruby_name: &str) -> String {
    match ruby_name {
        "StandardError" => "Exception".to_string(),
        "RuntimeError" => "Exception".to_string(),
        "ArgumentError" | "TypeError" | "NotImplementedError" | "NoMethodError"
        | "RangeError" | "IndexError" | "KeyError" | "Exception" => ruby_name.to_string(),
        _ => ruby_name.to_string(),
    }
}

/// Format a Crystal `require` path. `./relpath` for sibling or
/// descendant; `../relpath` for ancestor (no `./` prefix when the
/// path already starts with `..`).
fn crystal_require(from_dir: &Path, to_anchor: &str) -> String {
    let rel = relpath(from_dir, to_anchor);
    if rel.starts_with("..") {
        rel
    } else {
        format!("./{rel}")
    }
}

fn relpath(from_dir: &Path, to_anchor: &str) -> String {
    let from_parts: Vec<&str> = from_dir
        .to_str()
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to_anchor.split('/').filter(|s| !s.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from_parts.len() - common;
    let mut parts: Vec<&str> = std::iter::repeat("..").take(ups).collect();
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

pub(super) fn walk_const_paths(e: &Expr, out: &mut BTreeSet<Vec<String>>) {
    match &*e.node {
        ExprNode::Const { path } => {
            out.insert(path.iter().map(|s| s.as_str().to_string()).collect());
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk_const_paths(r, out);
            }
            for a in args {
                walk_const_paths(a, out);
            }
            if let Some(b) = block {
                walk_const_paths(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            walk_const_paths(fun, out);
            for a in args {
                walk_const_paths(a, out);
            }
            if let Some(b) = block {
                walk_const_paths(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_const_paths(k, out);
                walk_const_paths(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_const_paths(el, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_const_paths(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_const_paths(left, out);
            walk_const_paths(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            walk_const_paths(value, out);
            walk_const_paths(body, out);
        }
        ExprNode::Lambda { body, .. } => walk_const_paths(body, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_const_paths(cond, out);
            walk_const_paths(then_branch, out);
            walk_const_paths(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk_const_paths(scrutinee, out);
            for arm in arms {
                walk_const_paths(&arm.body, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk_const_paths(e, out);
            }
        }
        ExprNode::Assign { value, .. } => walk_const_paths(value, out),
        ExprNode::Yield { args } => {
            for a in args {
                walk_const_paths(a, out);
            }
        }
        ExprNode::Raise { value } => walk_const_paths(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            walk_const_paths(expr, out);
            walk_const_paths(fallback, out);
        }
        ExprNode::Return { value } => walk_const_paths(value, out),
        ExprNode::Super { args: Some(args) } => {
            for a in args {
                walk_const_paths(a, out);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk_const_paths(body, out);
            for r in rescues {
                walk_const_paths(&r.body, out);
            }
            if let Some(e) = else_branch {
                walk_const_paths(e, out);
            }
            if let Some(e) = ensure {
                walk_const_paths(e, out);
            }
        }
        ExprNode::Next { value: Some(v) } => walk_const_paths(v, out),
        ExprNode::MultiAssign { value, .. } => walk_const_paths(value, out),
        ExprNode::While { cond, body, .. } => {
            walk_const_paths(cond, out);
            walk_const_paths(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                walk_const_paths(b, out);
            }
            if let Some(e) = end {
                walk_const_paths(e, out);
            }
        }
        _ => {}
    }
}
