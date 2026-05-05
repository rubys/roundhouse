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
/// files (e.g. `inflector.rb`). Returns the bare method bodies; the
/// loader wraps them in the appropriate header/imports.
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
    let mut out = String::new();
    for (i, m) in methods.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&emit_method_impl(m));
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
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> EmittedFile {
    let name = lc.name.0.as_str();
    let out_dir = out_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(PathBuf::new);
    let self_anchor = out_path.with_extension("").to_string_lossy().into_owned();
    let mut s = String::new();

    // Parent + body-derived `require` headers. Crystal's `require
    // "./relpath"` is the analog of Ruby's `require_relative`. Anchor
    // computation is identical to Spinel's.
    let mut requires: Vec<String> = Vec::new();
    if let Some(parent) = lc.parent.as_ref() {
        if let Some(anchor) = require_path_for_parent(parent, app) {
            if anchor != self_anchor {
                requires.push(format!("./{}", relpath(&out_dir, &anchor)));
            }
        }
    }
    let mut const_paths: BTreeSet<Vec<String>> = BTreeSet::new();
    for m in &lc.methods {
        walk_const_paths(&m.body, &mut const_paths);
    }
    let mut body_requires: BTreeSet<String> = BTreeSet::new();
    for path in &const_paths {
        let first = match path.first() {
            Some(s) => s,
            None => continue,
        };
        if let Some((_, anchor)) = synthesized_siblings.iter().find(|(n, _)| n == first) {
            if anchor != &self_anchor {
                body_requires.insert(format!("./{}", relpath(&out_dir, anchor)));
                continue;
            }
        }
        if let Some(anchor) = require_path_for_body_const(path, app, name) {
            if anchor != self_anchor && !is_same_dir(&out_dir, &anchor) {
                body_requires.insert(format!("./{}", relpath(&out_dir, &anchor)));
            }
        }
    }
    requires.extend(body_requires);
    for r in &requires {
        writeln!(s, "require {r:?}").unwrap();
    }
    if !requires.is_empty() {
        writeln!(s).unwrap();
    }

    s.push_str(&render_class(lc));

    EmittedFile { path: out_path, content: s }
}

/// Render the `module ... end` / `class ... end` text for a single
/// LibraryClass. Used by `emit_library_class_decl` (with require
/// headers) and by `emit_library_class` (no headers — the caller
/// supplies them).
fn render_class(lc: &LibraryClass) -> String {
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
            Some(p) => writeln!(s, "{pad}class {last} < {}", p.0.as_str()).unwrap(),
            None => writeln!(s, "{pad}class {last}").unwrap(),
        }
    }

    for inc in &lc.includes {
        writeln!(s, "{body_pad}include {}", inc.0.as_str()).unwrap();
    }
    if !lc.includes.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    let mut first = true;
    for m in &lc.methods {
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
