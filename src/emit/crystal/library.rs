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
            is_async: false,
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
    // First pass: detect which ivars `initialize` directly assigns
    // (used both for ivar nilability AND property nilability —
    // Crystal's strict null check applies the same rule to both).
    let mut init_assigned: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for m in &lc.methods {
        if m.name.as_str() == "initialize"
            && matches!(m.receiver, crate::dialect::MethodReceiver::Instance)
        {
            collect_initialize_assignments(&m.body, &mut init_assigned);
        }
    }

    // Field names that the framework parent (`ActiveRecord::Base`)
    // already declares — every model extends ActiveRecord::Base
    // transitively, so re-declaring these in the subclass trips
    // Crystal's "instance variable already declared" error. Same set
    // TS uses (typescript.rs::INHERITED_FIELD_NAMES) — kept aligned
    // because both targets share the same ActiveRecord::Base shape.
    const INHERITED_FIELD_NAMES: &[&str] = &["id", "errors", "persisted", "destroyed"];
    let extends_active_record_base = matches!(lc.parent.as_ref(), Some(p) if {
        let raw = p.0.as_str();
        // Either explicitly `ActiveRecord::Base` OR an
        // ApplicationRecord-style intermediate that itself extends
        // ActiveRecord::Base. The fixture `ApplicationRecord` is
        // emitted as `class ApplicationRecord < ActiveRecord::Base`
        // (synthesized when missing); other models extend that.
        raw == "ActiveRecord::Base" || raw == "ApplicationRecord"
    });

    for m in &lc.methods {
        match m.kind {
            AccessorKind::AttributeReader => {
                // Crystal's `property` macro requires a plain
                // identifier — no `?` or `!` suffix. Predicate-named
                // attr_readers (`abstract?`) and bang-named ones don't
                // collapse; they emit as the explicit `def name? : T`
                // form instead.
                let mname = m.name.as_str();
                if mname.ends_with('?') || mname.ends_with('!') {
                    continue;
                }
                // Skip inherited fields — re-declaring would conflict
                // with the parent's declaration. Still mark as accessor
                // so the explicit getter/setter `def`s also drop (the
                // parent already provides them).
                if extends_active_record_base
                    && INHERITED_FIELD_NAMES.contains(&mname)
                {
                    accessor_method_names.insert(mname.to_string());
                    accessor_method_names.insert(format!("{mname}="));
                    continue;
                }
                let ty = match m.signature.as_ref() {
                    Some(crate::ty::Ty::Fn { ret, .. }) => super::ty::crystal_ty(ret),
                    _ => {
                        // No signature — `attr_accessor` synth without
                        // RBS annotation. Walk every method body in
                        // this class for an `@<mname> = <expr>`
                        // assignment and use the first non-Untyped
                        // type seen. The default-fallback `String` is
                        // wrong for accessors holding arrays / hashes
                        // (e.g. `errors : Array(String)`), so this
                        // ivar-driven inference is critical when the
                        // RBS sidecar doesn't cover the test class.
                        let target = format!("@{mname}");
                        let mut probe: indexmap::IndexMap<String, crate::ty::Ty> =
                            indexmap::IndexMap::new();
                        for other in &lc.methods {
                            collect_ivar_assignments(&other.body, &mut probe);
                        }
                        match probe.get(&target[1..]) {
                            Some(t) if !matches!(t, crate::ty::Ty::Untyped) => {
                                super::ty::crystal_ty(t)
                            }
                            _ => "String".to_string(),
                        }
                    }
                };
                // Append `?` only when initialize doesn't assign this
                // property — Crystal's strict null check requires
                // either every-init-path-assigns OR a nilable
                // declaration. attr_accessors that are populated
                // post-construct (controllers' `request_method`,
                // dispatch-set; views' yield body, etc.) need the
                // nilable form.
                let needs_nilable = !init_assigned.contains(mname)
                    && !ty.ends_with('?')
                    && ty != "Nil";
                let final_ty = if needs_nilable {
                    format!("{ty}?")
                } else {
                    ty
                };
                properties.push((mname.to_string(), final_ty));
                accessor_method_names.insert(mname.to_string());
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

    // `init_assigned` was already populated in the property pass.
    let initialize_assigned = init_assigned;

    // For modules whose methods are all class-level (Ruby's
    // `module_function` pattern — view_helpers.rb being the canonical
    // example), `@ivar` references in the method bodies are rewritten
    // to `@@ivar` (Crystal class variables, since metaclass instance
    // vars aren't allowed). The class-header declarations follow suit:
    // emit `@@name : T?` instead of `@name : T?`.
    let is_class_var_module = lc.is_module
        && !lc.methods.is_empty()
        && lc
            .methods
            .iter()
            .all(|m| matches!(m.receiver, crate::dialect::MethodReceiver::Class));
    let ivar_prefix = if is_class_var_module { "@@" } else { "@" };

    // Class header emit: `property` declarations first, then `@ivar`
    // declarations, then methods (with attr_reader/writer methods
    // skipped via `accessor_method_names`).
    let mut wrote_header_lines = false;
    for (name, ty) in &properties {
        // Same parent-aware default-value treatment as the bare-ivar
        // emit below: Crystal's strict null check applies across ALL
        // reachable initialize methods (including the parent's).
        // Subclasses that initialize @x in their own initialize but
        // inherit a parent initialize that doesn't would otherwise
        // trip the "indirect initialization" error. A type-appropriate
        // default at the property declaration site satisfies the
        // check uniformly.
        let default_clause = if !ty.ends_with('?') && lc.parent.is_some() {
            default_value_for_crystal_ty(ty)
                .map(|v| format!(" = {v}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        writeln!(s, "{body_pad}property {name} : {ty}{default_clause}").unwrap();
        wrote_header_lines = true;
    }
    // Class-var modules (`module_function` style — view_helpers.rb)
    // share state across `def self.X` methods. The Ruby source's
    // module-body `@var = {}` initializer is dropped at ingest time
    // (top-level statements outside `def`/constants don't survive),
    // so Crystal sees the class var only via in-method assignments —
    // making it nilable until the first call. `@@var[k] = v` then
    // fails: Crystal complains `[]=` is undefined for `Nil`. Emit
    // explicit `@@var : Hash(K, V) = {} of K => V` declarations,
    // deriving K/V from index-assignment sites in the method bodies
    // (`@@var[k] = v` carries `k.ty` and `v.ty` post body-typer).
    // Falls back to `Hash(String, String)` when no index assignment
    // is found — same default the empty-Hash literal emit picks.
    let mut classvar_hash_types: indexmap::IndexMap<String, (String, String)> =
        indexmap::IndexMap::new();
    if is_class_var_module {
        let mut classvar_index_types: indexmap::IndexMap<String, (crate::ty::Ty, crate::ty::Ty)> =
            indexmap::IndexMap::new();
        for m in &lc.methods {
            collect_classvar_index_types(&m.body, &mut classvar_index_types);
        }
        for (name, _ty) in &ivars {
            let (k_ty, v_ty) = classvar_index_types.get(name).cloned().unwrap_or_else(|| {
                (crate::ty::Ty::Str, crate::ty::Ty::Str)
            });
            let k_s = super::ty::crystal_ty(&k_ty);
            let v_s = super::ty::crystal_ty(&v_ty);
            writeln!(
                s,
                "{body_pad}@@{name} : Hash({k_s}, {v_s}) = {{}} of {k_s} => {v_s}",
            )
            .unwrap();
            wrote_header_lines = true;
            classvar_hash_types.insert(name.clone(), (k_s, v_s));
        }
    } else {
        for (name, ty) in &ivars {
            // Same inherited-field skip as the property pass.
            if extends_active_record_base
                && INHERITED_FIELD_NAMES.contains(&name.as_str())
            {
                continue;
            }
            let ty_s = super::ty::crystal_ty(ty);
            // Append `?` (nilable) only when initialize doesn't
            // assign this ivar directly. Crystal's strict null
            // checking requires either every-path-initializes or
            // a nilable declaration; declaring non-nilable an ivar
            // that's set later in another method (controllers'
            // action-set ivars are the canonical case) trips
            // a class-declaration error.
            let needs_nilable = !initialize_assigned.contains(name)
                && !ty_s.ends_with('?')
                && ty_s != "Nil";
            let final_ty = if needs_nilable {
                format!("{ty_s}?")
            } else {
                ty_s.clone()
            };
            // Crystal's strict null check applies across ALL reachable
            // initialize methods (including inherited ones). When the
            // class extends a parent that has its own initialize not
            // assigning this ivar, declaring non-nilable + assigning
            // only in the subclass's initialize trips the same error.
            // Emit a type-appropriate default value at the declaration
            // site so the parent's initialize path sees it pre-set —
            // analogous to Crystal's own pattern for `property x : T = <default>`.
            let default_clause = if !needs_nilable && lc.parent.is_some() {
                default_value_for_crystal_ty(&ty_s)
                    .map(|v| format!(" = {v}"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            writeln!(s, "{body_pad}{ivar_prefix}{name} : {final_ty}{default_clause}").unwrap();
            wrote_header_lines = true;
        }
    }
    if wrote_header_lines && lc.methods.iter().any(|m| !is_skipped_method(m, &accessor_method_names)) {
        writeln!(s).unwrap();
    }

    // For each instance ivar declared as a typed Hash (`@data :
    // Hash(K, V)`), build the rewrite pair so the methods loop
    // below can align `@<name> = {} of String => String` (emit_hash's
    // default for empty literals) with the declared Hash(K, V).
    // Same pattern the class-var loop above already handles for
    // `@@<name>`. Without this, the class header declares the
    // widened Hash type but `def initialize; @data = {}; end`
    // emits the default-typed empty literal and Crystal rejects.
    let mut ivar_hash_types: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for (name, ty) in &ivars {
        if let crate::ty::Ty::Hash { key, value } = ty {
            // Skip when the value type collapses to the default
            // `String` (Untyped/Var fall back there); the
            // unconditional rewrite would be a no-op.
            if !matches!(&**value, crate::ty::Ty::Untyped | crate::ty::Ty::Var { .. }) {
                ivar_hash_types.insert(
                    name.clone(),
                    (super::ty::crystal_ty(key), super::ty::crystal_ty(value)),
                );
            }
        }
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
        let mut body = emit_method_impl(m);
        // For class-var modules, post-process the emitted body to
        // align in-method empty-Hash assignments (`@@var = {} of
        // String => String` from emit_hash's default) with the
        // class-var declared type. `reset_slots!` style methods
        // need to reassign matching the declared `Hash(K, V)`; the
        // default `Hash(String, String)` would clash. Cheap string
        // replace — the pattern is unambiguous (literal `{} of
        // String => String` only emits as the empty-Hash default).
        if !classvar_hash_types.is_empty() {
            for (name, (k_s, v_s)) in &classvar_hash_types {
                let from = format!(
                    "@@{name} = {{}} of String => String",
                );
                let to = format!(
                    "@@{name} = {{}} of {k_s} => {v_s}",
                );
                if body.contains(&from) {
                    body = body.replace(&from, &to);
                }
            }
        }
        // Same realignment for instance ivars — see `ivar_hash_types`
        // above. `@data = {} of String => String` (emit_hash's
        // default) becomes `@data = {} of K => V` matching the
        // header declaration.
        if !ivar_hash_types.is_empty() {
            for (name, (k_s, v_s)) in &ivar_hash_types {
                let from = format!(
                    "@{name} = {{}} of String => String",
                );
                let to = format!(
                    "@{name} = {{}} of {k_s} => {v_s}",
                );
                if body.contains(&from) {
                    body = body.replace(&from, &to);
                }
            }
        }
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
        // hash AND carries a typed signature (the synthesizer attaches
        // `Ty::Fn` with `Hash[Symbol, Untyped]`). User-written
        // `initialize(attrs = {})` in test fixtures has no RBS sig
        // attached → `signature` is None — those still emit through
        // the normal path. Without the signature check, hand-written
        // model fixtures defining `initialize` for `create(attrs)`
        // would be silently dropped.
        let only_param = &m.params[0];
        if only_param.as_str() == "attrs" && m.signature.is_some() {
            return true;
        }
    }
    false
}

/// Return a Crystal literal that serves as the "empty" / zero value
/// for `ty_s` (an emitted Crystal type string). Used to satisfy
/// strict null-check on subclass ivars whose parent's initialize
/// doesn't assign them. Returns `None` for types that don't have an
/// obvious default — caller falls back to declaring without a
/// default (which leaves Crystal's null check active).
fn default_value_for_crystal_ty(ty_s: &str) -> Option<String> {
    match ty_s {
        "String" => Some(r#""""#.to_string()),
        "Int32" => Some("0".to_string()),
        "Int64" => Some("0_i64".to_string()),
        "Float32" => Some("0.0_f32".to_string()),
        "Float64" => Some("0.0".to_string()),
        "Bool" => Some("false".to_string()),
        _ => None,
    }
}

/// Walk an Expr collecting just the names of ivars assigned directly
/// inside `initialize` (or its top-level Seq). Used to decide whether
/// an ivar declaration needs the nilable `?` suffix in Crystal —
/// every-path-assigned ivars stay non-nilable. Conservative: doesn't
/// recurse into conditional branches or method bodies, which matches
/// Crystal's "directly initialized" rule.
fn collect_initialize_assignments(
    e: &Expr,
    out: &mut std::collections::HashSet<String>,
) {
    use crate::expr::LValue;
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, .. } => {
            out.insert(name.as_str().to_string());
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_initialize_assignments(e, out);
            }
        }
        _ => {}
    }
}

/// Walk an Expr collecting `@ivar[k] = v` (index-assign on Ivar) sites,
/// keyed by ivar name. Returns `(key_ty, value_ty)` pairs derived from
/// the post-typing IR — used by the class-var declaration emit to size
/// `@@var : Hash(K, V)` precisely. First match wins; multi-method
/// disagreement falls through to the first encountered shape (rare in
/// the framework runtime).
fn collect_classvar_index_types(
    e: &Expr,
    out: &mut indexmap::IndexMap<String, (crate::ty::Ty, crate::ty::Ty)>,
) {
    use crate::expr::LValue;
    if let ExprNode::Assign {
        target: LValue::Index { recv, index },
        value,
    } = &*e.node
    {
        if let ExprNode::Ivar { name } = &*recv.node {
            let key_ty = index.ty.clone().unwrap_or(crate::ty::Ty::Str);
            let val_ty = value.ty.clone().unwrap_or(crate::ty::Ty::Str);
            out.entry(name.as_str().to_string())
                .or_insert((key_ty, val_ty));
        }
    }
    // Also handle `Send { method: "[]=" , recv: Ivar, args: [k, v] }`
    // — Ruby's parser sometimes shapes `@var[k] = v` this way.
    if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*e.node {
        if method.as_str() == "[]=" && args.len() == 2 {
            if let ExprNode::Ivar { name } = &*recv.node {
                let key_ty = args[0].ty.clone().unwrap_or(crate::ty::Ty::Str);
                let val_ty = args[1].ty.clone().unwrap_or(crate::ty::Ty::Str);
                out.entry(name.as_str().to_string())
                    .or_insert((key_ty, val_ty));
            }
        }
    }
    visit_subexprs_for_classvar(e, |c| collect_classvar_index_types(c, out));
}

fn visit_subexprs_for_classvar(e: &Expr, mut f: impl FnMut(&Expr)) {
    use crate::expr::LValue;
    match &*e.node {
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                f(recv);
            }
            f(value);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                f(r);
            }
            for a in args {
                f(a);
            }
            if let Some(b) = block {
                f(b);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            f(fun);
            for a in args {
                f(a);
            }
            if let Some(b) = block {
                f(b);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k);
                f(v);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                f(el);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left);
            f(right);
        }
        ExprNode::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        ExprNode::Lambda { body, .. } => f(body),
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond);
            f(then_branch);
            f(else_branch);
        }
        ExprNode::Case { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                f(&arm.body);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                f(e);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                f(a);
            }
        }
        ExprNode::Raise { value } => f(value),
        ExprNode::RescueModifier { expr, fallback } => {
            f(expr);
            f(fallback);
        }
        ExprNode::Return { value } => f(value),
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            f(body);
            for r in rescues {
                f(&r.body);
            }
            if let Some(e) = else_branch {
                f(e);
            }
            if let Some(e) = ensure {
                f(e);
            }
        }
        ExprNode::While { cond, body, .. } => {
            f(cond);
            f(body);
        }
        ExprNode::Cast { value, .. } => f(value),
        _ => {}
    }
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
        // `@ivar[k] = v` — Ruby parses this as a `Send` to `[]=`
        // with the ivar as receiver. The ivar is a Hash whose value
        // type includes `v`'s type; widen the existing entry so
        // Crystal's strict typing picks up the actual stored types
        // instead of collapsing to the empty-literal default
        // (`@data = {}` typed as `Hash<Untyped, Untyped>` →
        // `Hash(String, String)` at emit). Catches the HWIA pattern
        // `@data[k.to_s] = normalize_value(v)`.
        ExprNode::Send { recv: Some(recv), method, args, .. }
            if method.as_str() == "[]=" && args.len() == 2 =>
        {
            if let ExprNode::Ivar { name } = &*recv.node {
                if let Some(v_ty) = &args[1].ty {
                    let key = name.as_str().to_string();
                    // Only widen when the existing entry is already
                    // Hash-typed. If the ivar was assigned a typed
                    // class instance (e.g. `@hash = HWIA.new(...)`),
                    // overwriting with `Hash<String, V>` would
                    // mis-declare the ivar — the class instance has
                    // its own `[]=` method that just happens to
                    // share the name. The widening exists to grow
                    // empty-Hash-literal types from observed
                    // assignments, not to retype class instances.
                    if let Some(crate::ty::Ty::Hash { key: k, value: existing }) = out.get(&key) {
                        let widened = crate::ty::Ty::Hash {
                            key: k.clone(),
                            value: Box::new(union_widen(existing, v_ty)),
                        };
                        out.insert(key, widened);
                    } else if !out.contains_key(&key) {
                        // Fresh Hash entry with the assigned value type.
                        out.insert(
                            key,
                            crate::ty::Ty::Hash {
                                key: Box::new(crate::ty::Ty::Str),
                                value: Box::new(v_ty.clone()),
                            },
                        );
                    }
                }
            }
            collect_ivar_assignments(recv, out);
            for a in args {
                collect_ivar_assignments(a, out);
            }
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


/// Widen `existing` to also accept `incoming`. If `existing` already
/// covers `incoming` (subset/equal), returns `existing` clone;
/// otherwise produces a `Ty::Union` joining the variants. Used by
/// the ivar-assignment walker to grow a Hash's value-type from
/// the empty-literal `Untyped` to the actual union of stored types.
fn union_widen(existing: &crate::ty::Ty, incoming: &crate::ty::Ty) -> crate::ty::Ty {
    use crate::ty::Ty;
    if matches!(existing, Ty::Untyped) {
        return incoming.clone();
    }
    if existing == incoming {
        return existing.clone();
    }
    let mut variants: Vec<Ty> = match existing {
        Ty::Union { variants } => variants.clone(),
        other => vec![other.clone()],
    };
    let incoming_variants: Vec<Ty> = match incoming {
        Ty::Union { variants } => variants.clone(),
        other => vec![other.clone()],
    };
    for v in incoming_variants {
        if !variants.contains(&v) {
            variants.push(v);
        }
    }
    if variants.len() == 1 {
        variants.into_iter().next().unwrap()
    } else {
        Ty::Union { variants }
    }
}

/// Map Ruby exception-class parents to their Crystal equivalents.
/// Ruby's `StandardError`/`RuntimeError` correspond to Crystal's
/// `Exception` (Crystal doesn't have a separate StandardError);
/// app-level parents (`Article < ApplicationRecord`) and the other
/// `*Error` classes pass through unchanged.
pub(super) fn crystal_parent_name(ruby_name: &str) -> String {
    match ruby_name {
        "StandardError" | "RuntimeError" => "Exception".to_string(),
        // Test parents collapse to the hand-written Crystal Minitest
        // analog. `runtime/crystal/test_helper.cr`'s `RoundhouseTest`
        // provides the assert_*/refute_* surface and a `macro
        // inherited` that registers each `test_*` method as a Spec
        // `it` block.
        "Minitest::Test"
        | "ActiveSupport::TestCase"
        | "ActionDispatch::IntegrationTest" => "RoundhouseTest".to_string(),
        _ => ruby_name.to_string(),
    }
}
