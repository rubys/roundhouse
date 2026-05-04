//! Parse a standalone Ruby source file (intended to hold runtime
//! library code authored in Ruby) into Roundhouse `MethodDef` values.
//!
//! This is the Ruby-body half of the runtime-extraction pipeline;
//! [`crate::rbs`] covers signatures. A later step marries the two: for
//! each method name, the body from here gets the signature from there.
//!
//! Scope: top-level `def`s and `def`s inside a single-level `module`/
//! `class` body. Required positional params only. Anything more exotic
//! (keyword args, rest/splat, blocks, nested scopes) is rejected with
//! `Err` rather than silently dropped, mirroring the RBS side.

use ruby_prism::{Node, parse};

use crate::dialect::{MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::ingest::ingest_expr;
use crate::rbs::parse_signatures;
use crate::span::Span;
use crate::ty::Ty;

const VIRTUAL_FILE: &str = "<runtime>";

/// Parse Ruby source and collect module/class-level constant
/// assignments whose value is a typeable literal. Patterns recognized:
///   `CONST = { k: v, ... }`
///   `CONST = { k: v, ... }.freeze`
///   `CONST = [a, b, ...]`
///   `CONST = [a, b, ...].freeze`
/// The returned map keys are the constant's last-segment name; values
/// are the inferred Ty (Hash[K, V] / Array[T] / etc.). Used by the
/// body-typer's `ExprNode::Const` arm so dispatch on a constant
/// (`STATUS_CODES.fetch(...)`) lands in the right primitive method
/// table.
pub fn parse_module_constants(source: &str) -> Result<std::collections::HashMap<Symbol, Ty>, String> {
    let result = parse(source.as_bytes());
    let mut out = std::collections::HashMap::new();
    if result.errors().count() > 0 {
        // Errors will surface elsewhere; return empty here so the caller
        // doesn't double-report.
        return Ok(out);
    }
    let root = result.node();
    walk_constants(&root, &mut out);
    Ok(out)
}

fn walk_constants(node: &Node<'_>, out: &mut std::collections::HashMap<Symbol, Ty>) {
    if let Some(program) = node.as_program_node() {
        for stmt in program.statements().body().iter() {
            collect_constant_from_stmt(&stmt, out);
        }
    } else if let Some(stmts) = node.as_statements_node() {
        for stmt in stmts.body().iter() {
            collect_constant_from_stmt(&stmt, out);
        }
    }
}

fn collect_constant_from_stmt(node: &Node<'_>, out: &mut std::collections::HashMap<Symbol, Ty>) {
    // Recurse into module/class bodies. Constants commonly live one
    // level inside `module Foo ... end` or `class Bar ... end`.
    if let Some(module) = node.as_module_node() {
        if let Some(body) = module.body() {
            walk_constants(&body, out);
        }
        return;
    }
    if let Some(class) = node.as_class_node() {
        if let Some(body) = class.body() {
            walk_constants(&body, out);
        }
        return;
    }
    // Top-level constant assignment: `CONST = literal[.freeze]?`.
    if let Some(write) = node.as_constant_write_node() {
        let name_bytes = write.name().as_slice();
        let Ok(name_str) = std::str::from_utf8(name_bytes) else { return };
        let value = write.value();
        if let Some(ty) = type_of_const_literal(&value) {
            out.insert(Symbol::new(name_str), ty);
        }
    }
}

/// Best-effort type inference for the right-hand side of a constant
/// assignment. Recognizes Hash and Array literals (typing element
/// types from the first key/value or first array element), with an
/// optional trailing `.freeze`. Falls back to None for unsupported
/// shapes — the body-typer's existing fallback still applies.
fn type_of_const_literal(node: &Node<'_>) -> Option<Ty> {
    // Strip `.freeze` if present — it's a no-op for typing.
    if let Some(call) = node.as_call_node() {
        let name = std::str::from_utf8(call.name().as_slice()).ok()?;
        if name == "freeze" {
            let inner = call.receiver()?;
            return type_of_const_literal(&inner);
        }
        return None;
    }
    if let Some(hash) = node.as_hash_node() {
        let first = hash.elements().iter().next();
        let Some(first) = first else {
            return Some(Ty::Hash {
                key: Box::new(Ty::Untyped),
                value: Box::new(Ty::Untyped),
            });
        };
        let assoc = first.as_assoc_node()?;
        let key_ty = type_of_literal_node(&assoc.key())?;
        let value_ty = type_of_literal_node(&assoc.value())?;
        return Some(Ty::Hash {
            key: Box::new(key_ty),
            value: Box::new(value_ty),
        });
    }
    if let Some(array) = node.as_array_node() {
        let first = array.elements().iter().next();
        let Some(first) = first else {
            return Some(Ty::Array { elem: Box::new(Ty::Untyped) });
        };
        let elem_ty = type_of_literal_node(&first)?;
        return Some(Ty::Array { elem: Box::new(elem_ty) });
    }
    None
}

fn type_of_literal_node(node: &Node<'_>) -> Option<Ty> {
    if node.as_integer_node().is_some() { return Some(Ty::Int); }
    if node.as_float_node().is_some() { return Some(Ty::Float); }
    if node.as_string_node().is_some() { return Some(Ty::Str); }
    if node.as_symbol_node().is_some() { return Some(Ty::Sym); }
    if node.as_true_node().is_some() || node.as_false_node().is_some() {
        return Some(Ty::Bool);
    }
    if node.as_nil_node().is_some() { return Some(Ty::Nil); }
    None
}

/// Parse Ruby source and extract every `def` it finds (at top level
/// and one level inside module/class bodies) as a `MethodDef`.
pub fn parse_methods(source: &str) -> Result<Vec<MethodDef>, String> {
    let result = parse(source.as_bytes());

    let errors: Vec<String> = result
        .errors()
        .map(|e| e.message().to_string())
        .collect();
    if !errors.is_empty() {
        return Err(format!("parse error: {}", errors.join("; ")));
    }

    let root = result.node();
    let mut out = Vec::new();
    walk_scope(&root, &mut out, None)?;
    Ok(out)
}

/// Parse Ruby source and its RBS sidecar, returning `MethodDef`s with
/// the RBS-derived `Ty::Fn` attached to `signature`. Every Ruby method
/// must have a matching RBS signature and vice versa; arities must match.
/// Method-body expressions are left with `ty: None` — sub-expression
/// typing is a separate step.
pub fn parse_methods_with_rbs(
    ruby_src: &str,
    rbs_src: &str,
) -> Result<Vec<MethodDef>, String> {
    parse_methods_with_rbs_in_ctx(
        ruby_src,
        rbs_src,
        &std::collections::HashMap::new(),
    )
}

/// Class-shape variant of `parse_methods_with_rbs`: ingest a whole
/// `.rb` file into per-class `LibraryClass` records (preserving parent,
/// includes, and is_module), attach the per-class RBS signatures from
/// the sidecar, and run the body-typer on each class's methods.
///
/// Class identity matching: `ingest_library_classes` keys by syntactic
/// last-segment (e.g. `RecordInvalid`); `parse_app_signatures` keys by
/// fully-qualified path (e.g. `ActiveRecord::RecordInvalid`). The match
/// here normalizes both sides to the last segment, which is sufficient
/// for the framework-runtime corpus (no name collisions across
/// runtime/ruby/).
///
/// Empty RBS for a class — including a class whose entire signature
/// comes from inheritance (e.g. `class RecordNotFound < StandardError`
/// with no body) — is allowed; methods that DO exist still need
/// matching signatures.
pub fn parse_library_with_rbs(
    ruby_src: &[u8],
    rbs_src: &str,
    file: &str,
) -> Result<Vec<crate::dialect::LibraryClass>, String> {
    use crate::ingest::ingest_library_classes;

    let mut library_classes = ingest_library_classes(ruby_src, file)
        .map_err(|e| format!("ingest_library_classes: {e:?}"))?;
    let sigs_by_class = crate::rbs::parse_app_signatures(rbs_src)?;

    // The class-grouped sig parser doesn't carry the `%a{abstract}`
    // annotation; the flat parser does. Use the flat result purely as
    // an abstract-name filter so per-class orphan checks skip
    // contract-only methods (e.g. base.rb's `[]` / `[]=`).
    let abstract_method_names: std::collections::HashSet<Symbol> =
        crate::rbs::parse_signatures(rbs_src)
            .map(|s| s.abstract_methods)
            .unwrap_or_default();

    // Normalize RBS sig keys to last-segment so they line up with
    // `ingest_library_classes`'s last-segment names.
    let sigs_by_last_seg: std::collections::HashMap<String, std::collections::HashMap<Symbol, Ty>> =
        sigs_by_class
            .into_iter()
            .map(|(cid, m)| {
                let last = cid
                    .0
                    .as_str()
                    .rsplit("::")
                    .next()
                    .unwrap_or("")
                    .to_string();
                (last, m)
            })
            .collect();

    // Step 1: marry signatures to methods (with arity check + abstract-
    // method orphan filter). Done up front so the class registry below
    // can be built from typed methods.
    // (Done inside the per-class loop below.)
    let constants = parse_module_constants(
        std::str::from_utf8(ruby_src).unwrap_or(""),
    )
    .unwrap_or_default();

    // Step 1: attach RBS signatures to each method, with arity check.
    // After this loop every method has its `signature` populated.
    for lc in &mut library_classes {
        let class_name = lc.name.0.as_str().to_string();
        let mut class_sigs = sigs_by_last_seg
            .get(&class_name)
            .cloned()
            .unwrap_or_default();

        for m in &mut lc.methods {
            let sig = class_sigs.remove(&m.name).ok_or_else(|| {
                format!(
                    "class `{}` method `{}` has no matching RBS signature",
                    class_name, m.name
                )
            })?;
            if let Ty::Fn { params, .. } = &sig {
                let rbs_arity = params
                    .iter()
                    .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                    .count();
                if rbs_arity != m.params.len() {
                    return Err(format!(
                        "class `{}` method `{}`: Ruby has {} positional param(s), RBS has {}",
                        class_name,
                        m.name,
                        m.params.len(),
                        rbs_arity
                    ));
                }
            } else {
                return Err(format!(
                    "class `{}` method `{}`: signature is not Ty::Fn",
                    class_name, m.name
                ));
            }
            m.signature = Some(sig);
        }

        // Drop abstract sigs from the orphan check. Subclass-overridden
        // contract methods declared `%a{abstract}` in the RBS have no
        // Ruby body in the base class by design (per the same convention
        // `parse_methods_with_rbs` already honors).
        for name in &abstract_method_names {
            class_sigs.remove(name);
        }
        if !class_sigs.is_empty() {
            let mut orphaned: Vec<String> = class_sigs.keys().map(|s| s.as_str().to_string()).collect();
            orphaned.sort();
            return Err(format!(
                "class `{}`: RBS signature(s) with no matching Ruby method: {}",
                class_name,
                orphaned.join(", "),
            ));
        }
    }

    // Step 2: build a class registry from the now-typed methods. The
    // body-typer dispatches `Send { recv: SelfRef, method: m }` against
    // self_ty's class entry, so without this registry self-method
    // calls resolve to `Ty::Untyped`.
    let mut class_registry: std::collections::HashMap<
        crate::ident::ClassId,
        crate::analyze::ClassInfo,
    > = std::collections::HashMap::new();
    // Use the shared `class_info_from_library_class` helper so kinds
    // (and the ivar-shadows-method reclassification — `def errors` paired
    // with `@errors = []` reads as a field, so its dispatch should not
    // force-parens) match what the model lowerer produces.
    for lc in &library_classes {
        let info = crate::lower::class_info_from_library_class(lc);
        class_registry.insert(lc.name.clone(), info);
    }

    // Step 3: body-type each method. Two-pass ivar typing mirroring the
    // module-flat path: pass A seeds nothing, pass B seeds ivar
    // bindings observed during pass A. With `annotate_self_dispatch`
    // set on the Ctx (below), the typer writes back `Some(SelfRef)`
    // on bare Sends that resolve through `class_registry[lc.name]`,
    // making the dispatch decision explicit in the IR. Per-target
    // emitters then render uniformly:
    // their return types flow into outer expressions (e.g. `errors`'s
    // `Array[String]` reaches `errors << "..."` so `<<` resolves to
    // `.push()` per the type-aware operator dispatch).
    let typer = crate::analyze::BodyTyper::new(&class_registry);
    for lc in &mut library_classes {
        let build_ctx = |m: &MethodDef,
                         ivars: &std::collections::HashMap<Symbol, Ty>|
         -> crate::analyze::Ctx {
            let mut ctx = crate::analyze::Ctx::default();
            if let Some(Ty::Fn { params, .. }) = &m.signature {
                for (param, p) in m.params.iter().zip(params.iter()) {
                    ctx.local_bindings.insert(param.name.clone(), p.ty.clone());
                }
            }
            ctx.self_ty = Some(Ty::Class {
                id: lc.name.clone(),
                args: vec![],
            });
            ctx.ivar_bindings = ivars.clone();
            ctx.constants = constants.clone();
            // Opt in to typer's self-dispatch annotation: bare Sends
            // that resolve through this class's methods get
            // `Some(SelfRef)` written back on their recv. Per-target
            // emit sees explicit self-receivers and renders accordingly
            // (Ruby: implicit, drop the prefix; TS: `this.method`).
            // Eliminates the prior `rewrite_bare_sends_to_self` pre-pass.
            ctx.annotate_self_dispatch = true;
            ctx
        };

        let empty_ivars: std::collections::HashMap<Symbol, Ty> =
            std::collections::HashMap::new();
        for m in &mut lc.methods {
            let ctx = build_ctx(m, &empty_ivars);
            typer.analyze_expr(&mut m.body, &ctx);
        }

        let mut flow_ivars: std::collections::HashMap<Symbol, Ty> =
            std::collections::HashMap::new();
        for m in &lc.methods {
            crate::analyze::extract_ivar_assignments(&m.body, &mut flow_ivars);
        }
        if !flow_ivars.is_empty() {
            let reseeded: std::collections::HashMap<Symbol, Ty> = flow_ivars
                .into_iter()
                .map(|(name, ty)| (name, Ty::Union { variants: vec![ty, Ty::Nil] }))
                .collect();
            for m in &mut lc.methods {
                let ctx = build_ctx(m, &reseeded);
                typer.analyze_expr(&mut m.body, &ctx);
            }
        }
    }

    Ok(library_classes)
}

/// Same as `parse_methods_with_rbs` but takes a pre-built class
/// registry — so cross-class method dispatch during body-typing can
/// resolve. Used by the runtime-sweep test, which builds a unified
/// registry from every `runtime/ruby/**/*.rbs` file before typing any
/// file's method bodies individually.
pub fn parse_methods_with_rbs_in_ctx(
    ruby_src: &str,
    rbs_src: &str,
    classes: &std::collections::HashMap<crate::ident::ClassId, crate::analyze::ClassInfo>,
) -> Result<Vec<MethodDef>, String> {
    let mut methods = parse_methods(ruby_src)?;
    let sigs = parse_signatures(rbs_src)?;

    let abstract_methods: std::collections::HashSet<String> = sigs
        .abstract_methods
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    let mut sig_map: std::collections::HashMap<String, Ty> = sigs
        .methods
        .into_iter()
        .map(|(n, ty)| (n.as_str().to_string(), ty))
        .collect();

    for m in &mut methods {
        let ty = sig_map.remove(m.name.as_str()).ok_or_else(|| {
            format!("method `{}` has no matching RBS signature", m.name)
        })?;

        if let Ty::Fn { params, .. } = &ty {
            // RBS injects a synthetic Block-kind param into Ty::Fn
            // when the signature declares `{ ... } -> T`. Ruby's flat
            // param list collects an `&block` only when explicitly
            // declared — implicit `yield` produces no Ruby-side
            // param. Filter the RBS Block param out of the arity
            // comparison: blocks are a separate axis from positionals
            // and keywords, and Ruby code that yields without
            // declaring `&block` is the common case (validates_*_of
            // is the canonical example).
            let rbs_arity = params
                .iter()
                .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                .count();
            if rbs_arity != m.params.len() {
                return Err(format!(
                    "method `{}`: Ruby has {} positional param(s), RBS has {}",
                    m.name,
                    m.params.len(),
                    rbs_arity
                ));
            }
        } else {
            return Err(format!("method `{}`: signature is not Ty::Fn", m.name));
        }

        m.signature = Some(ty);
    }

    // Drop signatures marked `%a{abstract}` from the orphan check.
    // Abstract methods declare a contract that subclasses fulfill;
    // the base class's .rb intentionally has no body for them.
    for name in &abstract_methods {
        sig_map.remove(name);
    }
    if !sig_map.is_empty() {
        let mut orphaned: Vec<String> = sig_map.keys().cloned().collect();
        orphaned.sort();
        return Err(format!(
            "RBS signature(s) with no matching Ruby method: {}",
            orphaned.join(", ")
        ));
    }

    // Two-pass flow-sensitive ivar typing, mirroring the model-side
    // analyzer (`src/analyze/mod.rs`):
    //
    // Pass A: type each body with only RBS-derived params + self_ty
    // seeded. Ivar reads resolve to `Ty::Var` (unknown), but ivar
    // *assignments* leave their value-expression typed, which Pass B
    // harvests.
    //
    // Pass B: gather every `@x = expr` across all method bodies,
    // wrap each in `Union<T, Nil>` (a first read can observe nil
    // before any assignment), seed `ivar_bindings`, and re-type.
    // Reads now resolve cleanly even when they lexically precede
    // the assignment (e.g. `@cache ||= compute` lowers to a `BoolOp`
    // whose left arm reads the unset ivar).
    //
    // Runtime code doesn't reference user classes today, so the
    // dispatch table is empty — the body-typer falls back to its
    // primitive method tables for everything.
    let typer = crate::analyze::BodyTyper::new(classes);

    // Extract module-level constants from the .rb so dispatch on
    // `STATUS_CODES.fetch(...)` etc. resolves through the constant's
    // typed value (Hash[Sym, Int]) rather than falling through as
    // `Ty::Class { STATUS_CODES }` to unknown.
    let constants = parse_module_constants(ruby_src).unwrap_or_default();

    let build_ctx = |m: &MethodDef,
                     ivars: &std::collections::HashMap<Symbol, Ty>|
     -> crate::analyze::Ctx {
        let mut ctx = crate::analyze::Ctx::default();
        if let Some(Ty::Fn { params, .. }) = &m.signature {
            for (param, p) in m.params.iter().zip(params.iter()) {
                ctx.local_bindings.insert(param.name.clone(), p.ty.clone());
            }
        }
        if let Some(enclosing) = &m.enclosing_class {
            ctx.self_ty = Some(Ty::Class {
                id: crate::ident::ClassId(enclosing.clone()),
                args: vec![],
            });
        }
        ctx.ivar_bindings = ivars.clone();
        ctx.constants = constants.clone();
        ctx
    };

    let empty_ivars: std::collections::HashMap<Symbol, Ty> =
        std::collections::HashMap::new();
    for m in &mut methods {
        let ctx = build_ctx(m, &empty_ivars);
        typer.analyze_expr(&mut m.body, &ctx);
    }

    let mut flow_ivars: std::collections::HashMap<Symbol, Ty> =
        std::collections::HashMap::new();
    for m in &methods {
        crate::analyze::extract_ivar_assignments(&m.body, &mut flow_ivars);
    }

    if !flow_ivars.is_empty() {
        let reseeded: std::collections::HashMap<Symbol, Ty> = flow_ivars
            .into_iter()
            .map(|(name, ty)| (name, Ty::Union { variants: vec![ty, Ty::Nil] }))
            .collect();
        for m in &mut methods {
            let ctx = build_ctx(m, &reseeded);
            typer.analyze_expr(&mut m.body, &ctx);
        }
    }

    Ok(methods)
}

fn walk_scope(
    node: &Node<'_>,
    out: &mut Vec<MethodDef>,
    enclosing: Option<&str>,
) -> Result<(), String> {
    // `module_function` (called bare in a module body) flips
    // subsequent `def`s in the same body into module-functions:
    // both an instance method AND a class method. For our targets
    // we only need the class-method form (callers spell it
    // `ViewHelpers.x(...)`), so promote those defs to
    // `MethodReceiver::Class`. Only direct `def` children of the
    // current scope get promoted — nested class bodies (e.g. a
    // FormBuilder class inside the same module) carry their own
    // method-receiver decisions through the recursive walk.
    let mut module_function_active = false;
    let mut visit = |stmt: &Node<'_>, out: &mut Vec<MethodDef>| -> Result<(), String> {
        if is_module_function_marker(stmt) {
            module_function_active = true;
            return Ok(());
        }
        let is_direct_def = stmt.as_def_node().is_some();
        let before = out.len();
        collect_from_stmt(stmt, out, enclosing)?;
        if module_function_active && is_direct_def {
            for m in &mut out[before..] {
                m.receiver = MethodReceiver::Class;
            }
        }
        Ok(())
    };
    if let Some(program) = node.as_program_node() {
        for stmt in program.statements().body().iter() {
            visit(&stmt, out)?;
        }
    } else if let Some(stmts) = node.as_statements_node() {
        for stmt in stmts.body().iter() {
            visit(&stmt, out)?;
        }
    }
    Ok(())
}

/// True when `node` is a bare `module_function` call (no receiver,
/// no args, no block) — the marker that flips subsequent defs in
/// the same module body to module-functions.
fn is_module_function_marker(node: &Node<'_>) -> bool {
    let Some(call) = node.as_call_node() else { return false };
    if call.receiver().is_some() {
        return false;
    }
    if call.arguments().is_some() {
        return false;
    }
    if call.block().is_some() {
        return false;
    }
    let Ok(name) = std::str::from_utf8(call.name().as_slice()) else {
        return false;
    };
    name == "module_function"
}

fn collect_from_stmt(
    node: &Node<'_>,
    out: &mut Vec<MethodDef>,
    enclosing: Option<&str>,
) -> Result<(), String> {
    if let Some(def) = node.as_def_node() {
        out.push(method_def_from(&def, enclosing)?);
        return Ok(());
    }
    // attr_reader / attr_writer / attr_accessor lower at parse time
    // to synthetic getter/setter MethodDef pairs. The RBS sidecar
    // declares the per-attr signatures (`def id: () -> Integer`); the
    // synthetic body here exists to satisfy the orphan check and to
    // give the body-typer something concrete to type. Body shape:
    //   def attr; @attr; end           (reader)
    //   def attr=(v); @attr = v; end   (writer)
    if let Some(call) = node.as_call_node() {
        if call.receiver().is_none() {
            let name_bytes = call.name().as_slice();
            if let Ok(name_str) = std::str::from_utf8(name_bytes) {
                let (mk_reader, mk_writer) = match name_str {
                    "attr_reader" => (true, false),
                    "attr_writer" => (false, true),
                    "attr_accessor" => (true, true),
                    _ => (false, false),
                };
                if mk_reader || mk_writer {
                    if let Some(args) = call.arguments() {
                        for arg in args.arguments().iter() {
                            let Some(sym) = arg.as_symbol_node() else { continue };
                            let Some(loc) = sym.value_loc() else { continue };
                            let attr_name = std::str::from_utf8(loc.as_slice())
                                .map_err(|_| "attr name not UTF-8".to_string())?;
                            if mk_reader {
                                out.push(synthesize_reader(attr_name, enclosing));
                            }
                            if mk_writer {
                                out.push(synthesize_writer(attr_name, enclosing));
                            }
                        }
                    }
                    return Ok(());
                }
            }
        }
    }
    // `class << self ... end` — singleton-class block. Recurse into
    // its body in the same enclosing scope, then promote every method
    // collected inside to `MethodReceiver::Class`. Covers both
    // `def self.x` (already Class) and `attr_*` lowerings (default
    // Instance), so e.g. `class << self; attr_accessor :adapter; end`
    // produces module-level `adapter` / `adapter=` class methods.
    if let Some(sc) = node.as_singleton_class_node() {
        if let Some(body) = sc.body() {
            let before = out.len();
            walk_scope(&body, out, enclosing)?;
            for m in &mut out[before..] {
                m.receiver = MethodReceiver::Class;
            }
        }
        return Ok(());
    }
    if let Some(module) = node.as_module_node() {
        let name_bytes = module.name().as_slice();
        let name_str = std::str::from_utf8(name_bytes)
            .map_err(|_| "module name is not UTF-8".to_string())?;
        if let Some(body) = module.body() {
            walk_scope(&body, out, Some(name_str))?;
        }
        return Ok(());
    }
    if let Some(class) = node.as_class_node() {
        let name_bytes = class.name().as_slice();
        let name_str = std::str::from_utf8(name_bytes)
            .map_err(|_| "class name is not UTF-8".to_string())?;
        if let Some(body) = class.body() {
            walk_scope(&body, out, Some(name_str))?;
        }
        return Ok(());
    }
    Ok(())
}

/// Synthesize `def <attr>; @<attr>; end`. Body is a single Ivar
/// read; the RBS sidecar's `def <attr>: () -> T` provides the type.
fn synthesize_reader(attr: &str, enclosing: Option<&str>) -> MethodDef {
    let name = Symbol::new(attr);
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Ivar { name: name.clone() },
    );
    MethodDef {
        name,
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: EffectSet::pure(),
        enclosing_class: enclosing.map(Symbol::new),
        kind: crate::dialect::AccessorKind::AttributeReader,
    }
}

/// Synthesize `def <attr>=(value); @<attr> = value; end`. Body is
/// `Assign { target: Ivar(attr), value: Var(value) }`. The RBS
/// sidecar's `def <attr>=: (T) -> T` provides the type.
fn synthesize_writer(attr: &str, enclosing: Option<&str>) -> MethodDef {
    let attr_sym = Symbol::new(attr);
    let setter_name = Symbol::new(&format!("{attr}="));
    let value_param = Symbol::new("value");
    let value_read = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: crate::ident::VarId(0),
            name: value_param.clone(),
        },
    );
    let body = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: attr_sym },
            value: value_read,
        },
    );
    MethodDef {
        name: setter_name,
        receiver: MethodReceiver::Instance,
        params: vec![Param::positional(value_param)],
        body,
        signature: None,
        effects: EffectSet::pure(),
        enclosing_class: enclosing.map(Symbol::new),
        kind: crate::dialect::AccessorKind::AttributeWriter,
    }
}

fn method_def_from(
    def: &ruby_prism::DefNode<'_>,
    enclosing: Option<&str>,
) -> Result<MethodDef, String> {
    let name_bytes = def.name().as_slice();
    let name = Symbol::new(
        std::str::from_utf8(name_bytes)
            .map_err(|_| "method name is not UTF-8".to_string())?,
    );

    let receiver = if def.receiver().is_some() {
        MethodReceiver::Class
    } else {
        MethodReceiver::Instance
    };

    let params = method_params(def, name.as_str())?;

    let body = match def.body() {
        Some(b) => ingest_expr(&b, VIRTUAL_FILE).map_err(|e| format!("in `{name}`: {e}"))?,
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(MethodDef {
        name,
        receiver,
        params,
        body,
        signature: None,
        effects: EffectSet::pure(),
        enclosing_class: enclosing.map(Symbol::new),
        // Source-defined `def` from runtime_src — Method by default.
        // Pattern-matching for attr_reader-shaped bodies could refine
        // this, but `attr_*` calls go through synthesize_reader/writer
        // above with explicit kinds; bare `def` is overwhelmingly Method.
        kind: crate::dialect::AccessorKind::Method,
    })
}

/// Collect every parameter's name, in source order, across all kinds:
/// required positional, optional positional, rest (`*args`), post-rest,
/// required keyword, optional keyword, kwargs (`**opts`), and block
/// (`&block`). Anonymous forms (`*`, `**`, `&`) are skipped.
///
/// Returned list is flat — no kind distinction preserved. The RBS
/// signature's `Ty::Fn` encodes the per-position kind; the arity check
/// in `parse_methods_with_rbs_in_ctx` ensures same-length alignment,
/// and the body-typer seeds local bindings by position-zipping names
/// to signature params.
fn method_params(def: &ruby_prism::DefNode<'_>, method_name: &str) -> Result<Vec<Param>, String> {
    let Some(params_node) = def.parameters() else {
        return Ok(Vec::new());
    };

    let mut names = Vec::new();

    // Required positional: `def foo(a, b)`.
    for req in params_node.requireds().iter() {
        let rp = req.as_required_parameter_node().ok_or_else(|| {
            format!("method `{method_name}`: unexpected required-parameter shape")
        })?;
        names.push(Param::positional(Symbol::new(decode_utf8(rp.name().as_slice(), method_name)?)));
    }

    // Optional positional: `def foo(a = 1)`.
    for opt in params_node.optionals().iter() {
        let op = opt.as_optional_parameter_node().ok_or_else(|| {
            format!("method `{method_name}`: unexpected optional-parameter shape")
        })?;
        names.push(Param::positional(Symbol::new(decode_utf8(op.name().as_slice(), method_name)?)));
    }

    // Rest/splat: `*args`. Anonymous `*` has no name — skip.
    if let Some(rest) = params_node.rest() {
        if let Some(rp) = rest.as_rest_parameter_node() {
            if let Some(loc) = rp.name() {
                names.push(Param::positional(Symbol::new(decode_utf8(loc.as_slice(), method_name)?)));
            }
        }
        // ImplicitRestNode (shorthand `def foo(a, *)`) has no name.
    }

    // Post-rest required positional: `def foo(*rest, a, b)`.
    for post in params_node.posts().iter() {
        let pp = post.as_required_parameter_node().ok_or_else(|| {
            format!("method `{method_name}`: unexpected post-required-parameter shape")
        })?;
        names.push(Param::positional(Symbol::new(decode_utf8(pp.name().as_slice(), method_name)?)));
    }

    // Keywords (required and optional): `def foo(a:, b: 1)`.
    for kw in params_node.keywords().iter() {
        if let Some(rkp) = kw.as_required_keyword_parameter_node() {
            names.push(Param::positional(Symbol::new(decode_utf8(rkp.name().as_slice(), method_name)?)));
        } else if let Some(okp) = kw.as_optional_keyword_parameter_node() {
            names.push(Param::positional(Symbol::new(decode_utf8(okp.name().as_slice(), method_name)?)));
        } else {
            return Err(format!(
                "method `{method_name}`: unexpected keyword-parameter shape"
            ));
        }
    }

    // Kwargs splat: `**opts`. `**nil` explicitly forbids kwargs and has
    // no name — skip it.
    if let Some(krest) = params_node.keyword_rest() {
        if let Some(krp) = krest.as_keyword_rest_parameter_node() {
            if let Some(loc) = krp.name() {
                names.push(Param::positional(Symbol::new(decode_utf8(loc.as_slice(), method_name)?)));
            }
        }
        // NoKeywordsParameterNode (`**nil`) — skip.
    }

    // Block: `&block`. Anonymous `&` has no name — skip.
    if let Some(block) = params_node.block() {
        if let Some(loc) = block.name() {
            names.push(Param::positional(Symbol::new(decode_utf8(loc.as_slice(), method_name)?)));
        }
    }

    Ok(names)
}

fn decode_utf8<'a>(bytes: &'a [u8], method_name: &str) -> Result<&'a str, String> {
    std::str::from_utf8(bytes)
        .map_err(|_| format!("method `{method_name}`: param name is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{ExprNode, InterpPart, Literal};

    fn parse_one(src: &str) -> MethodDef {
        let mut methods = parse_methods(src).expect("parses");
        assert_eq!(methods.len(), 1, "expected exactly one method");
        methods.remove(0)
    }

    #[test]
    fn toplevel_def_is_found() {
        let src = "def pluralize(count, word)\n  count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\nend\n";
        let m = parse_one(src);
        assert_eq!(m.name.as_str(), "pluralize");
        assert_eq!(m.receiver, MethodReceiver::Instance);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["count", "word"]
        );
    }

    #[test]
    fn module_nested_def_is_found() {
        let src = "module Inflector\n  def pluralize(count, word)\n    \"#{count} #{word}\"\n  end\nend\n";
        let m = parse_one(src);
        assert_eq!(m.name.as_str(), "pluralize");
        assert_eq!(m.params.len(), 2);
    }

    #[test]
    fn class_nested_def_is_found() {
        let src = "class Inflector\n  def f\n    1\n  end\nend\n";
        let m = parse_one(src);
        assert_eq!(m.name.as_str(), "f");
        assert!(m.params.is_empty());
    }

    #[test]
    fn self_receiver_is_class_kind() {
        let src = "module M\n  def self.f\n    1\n  end\nend\n";
        let m = parse_one(src);
        assert_eq!(m.receiver, MethodReceiver::Class);
    }

    #[test]
    fn pluralize_body_has_conditional_shape() {
        let src = "def pluralize(count, word)\n  count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\nend\n";
        let m = parse_one(src);

        let (cond, then_branch, else_branch) = match *m.body.node {
            ExprNode::If {
                cond,
                then_branch,
                else_branch,
            } => (cond, then_branch, else_branch),
            other => panic!("expected If at body, got {other:?}"),
        };

        // cond: count == 1
        match *cond.node {
            ExprNode::Send { method, .. } => assert_eq!(method.as_str(), "=="),
            other => panic!("expected `==` send in cond, got {other:?}"),
        }

        // Then branch: "1 #{word}"
        match *then_branch.node {
            ExprNode::StringInterp { parts } => {
                assert!(has_literal_text(&parts, "1 "), "then-branch missing `1 `");
                assert!(has_expr_var(&parts, "word"), "then-branch missing `word`");
            }
            other => panic!("expected StringInterp in then-branch, got {other:?}"),
        }

        // Else branch: "#{count} #{word}s"
        match *else_branch.node {
            ExprNode::StringInterp { parts } => {
                assert!(has_expr_var(&parts, "count"), "else-branch missing `count`");
                assert!(has_expr_var(&parts, "word"), "else-branch missing `word`");
                assert!(has_literal_text(&parts, "s"), "else-branch missing trailing `s`");
            }
            other => panic!("expected StringInterp in else-branch, got {other:?}"),
        }
    }

    fn has_literal_text(parts: &[InterpPart], needle: &str) -> bool {
        parts.iter().any(|p| match p {
            InterpPart::Text { value } => value.contains(needle),
            _ => false,
        })
    }

    fn has_expr_var(parts: &[InterpPart], var: &str) -> bool {
        parts.iter().any(|p| match p {
            InterpPart::Expr { expr } => matches!(
                &*expr.node,
                ExprNode::Var { name, .. } if name.as_str() == var
            ),
            _ => false,
        })
    }

    #[test]
    fn multiple_defs_in_order() {
        let src = "def a; 1; end\ndef b; 2; end\n";
        let methods = parse_methods(src).expect("parses");
        assert_eq!(
            methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn integer_literal_body_roundtrips() {
        let src = "def f\n  42\nend\n";
        let m = parse_one(src);
        assert!(matches!(
            &*m.body.node,
            ExprNode::Lit {
                value: Literal::Int { value: 42 }
            }
        ));
    }

    #[test]
    fn attr_reader_lowers_to_getter() {
        let src = "class C\n  attr_reader :name\nend\n";
        let methods = parse_methods(src).expect("parses");
        assert_eq!(methods.len(), 1);
        let m = &methods[0];
        assert_eq!(m.name.as_str(), "name");
        assert!(m.params.is_empty());
        assert!(matches!(
            &*m.body.node,
            ExprNode::Ivar { name } if name.as_str() == "name"
        ));
    }

    #[test]
    fn attr_writer_lowers_to_setter() {
        let src = "class C\n  attr_writer :name\nend\n";
        let methods = parse_methods(src).expect("parses");
        assert_eq!(methods.len(), 1);
        let m = &methods[0];
        assert_eq!(m.name.as_str(), "name=");
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name.as_str(), "value");
    }

    #[test]
    fn attr_accessor_lowers_to_getter_and_setter() {
        let src = "class C\n  attr_accessor :name\nend\n";
        let methods = parse_methods(src).expect("parses");
        assert_eq!(methods.len(), 2);
        assert_eq!(methods[0].name.as_str(), "name");
        assert_eq!(methods[1].name.as_str(), "name=");
    }

    #[test]
    fn attr_accessor_multi_arg_lowers_per_attr() {
        let src = "class C\n  attr_accessor :a, :b\nend\n";
        let methods = parse_methods(src).expect("parses");
        let names: Vec<_> = methods.iter().map(|m| m.name.as_str().to_string()).collect();
        assert_eq!(names, vec!["a", "a=", "b", "b="]);
    }

    #[test]
    fn multi_statement_body_is_sequenced() {
        let src = "def f\n  1\n  2\nend\n";
        let m = parse_one(src);
        let exprs = match *m.body.node {
            ExprNode::Seq { exprs } => exprs,
            other => panic!("expected Seq for multi-stmt body, got {other:?}"),
        };
        assert_eq!(exprs.len(), 2);
    }

    #[test]
    fn parse_error_surfaces() {
        let err = parse_methods("def f(").unwrap_err();
        assert!(err.contains("parse error"), "unexpected error: {err}");
    }

    #[test]
    fn keyword_params_collected() {
        let src = "def f(a:, b: 1)\n  1\nend\n";
        let m = parse_one(src);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
            "keyword param names preserved in order"
        );
    }

    #[test]
    fn splat_params_collected() {
        let src = "def f(*args)\n  1\nend\n";
        let m = parse_one(src);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["args"]
        );
    }

    #[test]
    fn block_params_collected() {
        let src = "def f(&blk)\n  1\nend\n";
        let m = parse_one(src);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["blk"]
        );
    }

    #[test]
    fn optional_params_collected() {
        let src = "def f(a = 1)\n  a\nend\n";
        let m = parse_one(src);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    #[test]
    fn mixed_param_kinds_in_source_order() {
        let src = "def f(a, b = 1, *rest, c, d:, e: 2, **opts, &blk)\n  a\nend\n";
        let m = parse_one(src);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "rest", "c", "d", "e", "opts", "blk"],
        );
    }

    #[test]
    fn anonymous_splat_and_block_skipped() {
        // `*` and `&` without names are positional/block anonymous
        // forwards. No name to capture; kept out of the params list.
        let src = "def f(a, *, &)\n  a\nend\n";
        let m = parse_one(src);
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    #[test]
    fn def_without_params_or_body() {
        let src = "def f\nend\n";
        let m = parse_one(src);
        assert!(m.params.is_empty());
        assert!(matches!(&*m.body.node, ExprNode::Seq { exprs } if exprs.is_empty()));
    }

    // ── parse_methods_with_rbs ──────────────────────────────────────

    use crate::ty::{Param, ParamKind};

    const PLURALIZE_RB: &str =
        "module Inflector\n  def pluralize(count, word)\n    count == 1 ? \"1 #{word}\" : \"#{count} #{word}s\"\n  end\nend\n";
    const PLURALIZE_RBS: &str =
        "module Inflector\n  def pluralize: (Integer, String) -> String\nend\n";

    #[test]
    fn marrying_attaches_signature() {
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        assert_eq!(methods.len(), 1);
        let m = &methods[0];
        assert_eq!(m.name.as_str(), "pluralize");

        let sig = m.signature.as_ref().expect("signature attached");
        let Ty::Fn { params, ret, .. } = sig else {
            panic!("expected Ty::Fn, got {sig:?}");
        };
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].ty, Ty::Int);
        assert_eq!(params[1].ty, Ty::Str);
        assert_eq!(**ret, Ty::Str);

        // Param kinds come from RBS (Required in this case).
        assert!(params.iter().all(|p: &Param| p.kind == ParamKind::Required));
    }

    #[test]
    fn ruby_param_names_coexist_with_rbs_types() {
        // RBS has anonymous positionals; Ruby param names should survive.
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        let m = &methods[0];
        assert_eq!(
            m.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["count", "word"]
        );
    }

    #[test]
    fn ruby_method_missing_signature_errors() {
        let ruby = "def foo\n  1\nend\n";
        let rbs = "module M\nend\n";
        let err = parse_methods_with_rbs(ruby, rbs).unwrap_err();
        assert!(
            err.contains("foo") && err.contains("no matching RBS"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn orphan_rbs_signature_errors() {
        let ruby = "def foo\n  1\nend\n";
        let rbs = "module M\n  def foo: () -> Integer\n  def bar: () -> String\nend\n";
        let err = parse_methods_with_rbs(ruby, rbs).unwrap_err();
        assert!(
            err.contains("no matching Ruby method") && err.contains("bar"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn arity_mismatch_errors() {
        let ruby = "def f(a, b)\n  1\nend\n";
        let rbs = "module M\n  def f: (Integer) -> Integer\nend\n";
        let err = parse_methods_with_rbs(ruby, rbs).unwrap_err();
        assert!(
            err.contains("2 positional param") && err.contains("RBS has 1"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn multi_method_marrying_preserves_ruby_order() {
        let ruby = "module M\n  def b\n    1\n  end\n  def a\n    \"x\"\n  end\nend\n";
        let rbs = "module M\n  def a: () -> String\n  def b: () -> Integer\nend\n";
        let methods = parse_methods_with_rbs(ruby, rbs).expect("types");
        // Ruby order: b, a
        assert_eq!(
            methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        // And each has its own signature.
        let b_sig = methods[0].signature.as_ref().unwrap();
        let a_sig = methods[1].signature.as_ref().unwrap();
        assert!(matches!(b_sig, Ty::Fn { ret, .. } if **ret == Ty::Int));
        assert!(matches!(a_sig, Ty::Fn { ret, .. } if **ret == Ty::Str));
    }

    #[test]
    fn empty_ruby_and_empty_rbs_yields_empty() {
        let methods = parse_methods_with_rbs("", "").expect("types");
        assert!(methods.is_empty());
    }

    #[test]
    fn ruby_parse_error_surfaces_through_marrying() {
        let err = parse_methods_with_rbs("def f(", "module M\nend\n").unwrap_err();
        assert!(err.contains("parse error"), "unexpected: {err}");
    }

    #[test]
    fn rbs_parse_error_surfaces_through_marrying() {
        let err = parse_methods_with_rbs("", "class { end").unwrap_err();
        assert!(!err.is_empty());
    }

    // ── body-typer integration ──────────────────────────────────────

    fn find_var_ty(e: &crate::expr::Expr, name: &str) -> Option<Ty> {
        // Walk the tree looking for `Var { name }` and return its `.ty`.
        match &*e.node {
            ExprNode::Var { name: n, .. } if n.as_str() == name => e.ty.clone(),
            ExprNode::If { cond, then_branch, else_branch } => find_var_ty(cond, name)
                .or_else(|| find_var_ty(then_branch, name))
                .or_else(|| find_var_ty(else_branch, name)),
            ExprNode::Send { recv, args, .. } => {
                if let Some(r) = recv {
                    if let Some(t) = find_var_ty(r, name) {
                        return Some(t);
                    }
                }
                args.iter().find_map(|a| find_var_ty(a, name))
            }
            ExprNode::StringInterp { parts } => parts.iter().find_map(|p| match p {
                crate::expr::InterpPart::Expr { expr } => find_var_ty(expr, name),
                _ => None,
            }),
            ExprNode::Seq { exprs } => exprs.iter().find_map(|e| find_var_ty(e, name)),
            _ => None,
        }
    }

    #[test]
    fn body_typer_populates_param_refs_with_signature_types() {
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        let m = &methods[0];
        // `count` is used in the cond (`count == 1`) and in the else-branch
        // interpolation (`"#{count} ..."`); both should resolve to Int.
        assert_eq!(find_var_ty(&m.body, "count"), Some(Ty::Int));
        // `word` is used in both branches; should resolve to Str.
        assert_eq!(find_var_ty(&m.body, "word"), Some(Ty::Str));
    }

    #[test]
    fn body_typer_populates_literal_and_interp_types() {
        let methods = parse_methods_with_rbs(PLURALIZE_RB, PLURALIZE_RBS).expect("types");
        let m = &methods[0];
        // The If as a whole unions its branches (both StringInterp → Str).
        assert_eq!(m.body.ty.as_ref(), Some(&Ty::Str));
    }
}
