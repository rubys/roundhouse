//! Integration-level invariant: the functions the emitter produces
//! from runtime/ruby/*.rb + *.rbs MUST appear verbatim in the
//! corresponding per-target runtime files.
//!
//! This is what makes the Ruby source the source of truth. Hand-edits
//! to the target runtime files without updating the Ruby/RBS source
//! will fail this test — the way `pluralize` changes is by editing
//! runtime/ruby/inflector.rb and re-running CI, not by touching
//! runtime/python/view_helpers.py directly.
//!
//! For now only Python is covered. TypeScript / Crystal / Go / Rust /
//! Elixir join as their emit_method gains the single standalone-fn
//! entry point. Each addition is ~5 lines in this file.

use std::fs;
use std::path::Path;

use roundhouse::analyze::ClassInfo;
use roundhouse::dialect::MethodDef;
use roundhouse::expr::{Expr, ExprNode, InterpPart};
use roundhouse::ident::ClassId;
use roundhouse::rbs::{parse_app_includes, parse_app_signatures};
use roundhouse::runtime_src::{parse_methods_with_rbs, parse_methods_with_rbs_in_ctx};
use roundhouse::ty::Ty;

fn load_typed(name: &str) -> Vec<MethodDef> {
    let ruby = fs::read_to_string(Path::new("runtime/ruby").join(format!("{name}.rb")))
        .expect("runtime/ruby/<name>.rb exists");
    let rbs = fs::read_to_string(Path::new("runtime/ruby").join(format!("{name}.rbs")))
        .expect("runtime/ruby/<name>.rbs exists");
    parse_methods_with_rbs(&ruby, &rbs).expect("Ruby+RBS parses and types cleanly")
}

fn pluralize_method() -> MethodDef {
    let methods = load_typed("inflector");
    methods
        .into_iter()
        .find(|m| m.name.as_str() == "pluralize")
        .expect("inflector.rb defines pluralize")
}

fn assert_emitted_lives_in(emitted: &str, file_path: &str) {
    let file = fs::read_to_string(file_path).unwrap_or_else(|_| panic!("{file_path} exists"));
    // Target runtime files typically nest the function inside a
    // module, so compare line-by-line modulo leading whitespace: the
    // emitter output must appear as a consecutive run of file lines
    // with only their indentation removed.
    let emitted_lines: Vec<&str> = emitted.lines().map(str::trim_start).collect();
    let file_lines: Vec<&str> = file.lines().map(str::trim_start).collect();
    let found = file_lines
        .windows(emitted_lines.len())
        .any(|w| w == emitted_lines.as_slice());
    assert!(
        found,
        "{file_path} does not contain the emitted function.\n\
         Expected (from runtime/ruby/inflector.rb + .rbs, compared modulo indent):\n\
         ----\n{emitted}----\n\
         If the emitter is now the source of truth, the runtime file must be \
         updated to match; if instead the runtime file was edited deliberately, \
         the Ruby/RBS source needs the same edit."
    );
}

#[test]
fn inflector_pluralize_lives_in_runtime_python() {
    let emitted = roundhouse::emit::python::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/python/view_helpers.py");
}

#[test]
fn inflector_pluralize_lives_in_runtime_crystal() {
    let emitted = roundhouse::emit::crystal::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/crystal/view_helpers.cr");
}

#[test]
fn inflector_pluralize_lives_in_runtime_rust() {
    let emitted = roundhouse::emit::rust::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/rust/view_helpers.rs");
}

#[test]
fn inflector_pluralize_lives_in_runtime_elixir() {
    let emitted = roundhouse::emit::elixir::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/elixir/view_helpers.ex");
}

#[test]
fn inflector_pluralize_lives_in_runtime_go() {
    let emitted = roundhouse::emit::go::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/go/view_helpers.go");
}

// ── full-typing invariant ───────────────────────────────────────────

/// Enumerate every `*.rb` under runtime/ruby/, recursively, and return
/// its stem path relative to runtime/ruby/ (without extension). Sweeps
/// both top-level files (inflector, active_record) and framework
/// library code (active_record/base, active_record/validations, etc.).
///
/// Excludes `runtime/ruby/test/` (CRuby test scaffolding, not framework
/// runtime code) and any dot-directories.
fn runtime_ruby_stems() -> Vec<String> {
    let root = Path::new("runtime/ruby");
    let mut out: Vec<String> = Vec::new();
    walk_ruby_files(root, root, &mut out);
    out.sort();
    out
}

fn walk_ruby_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|_| panic!("read_dir {dir:?}")) {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "test" {
                continue;
            }
            walk_ruby_files(root, &path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rb") {
            let rel = path.strip_prefix(root).expect("path under root");
            let rel_stem = rel.with_extension("");
            if let Some(s) = rel_stem.to_str() {
                out.push(s.to_string());
            }
        }
    }
}

/// Walk the typed IR and count every sub-expression whose type is
/// `Ty::Untyped` (RBS-declared gradual escape). Bar B tracker —
/// these nodes pass the strict Bar A test but block strict-target
/// emission (Rust). Non-recursive in itself; calls
/// `count_gradual_recurse` to descend.
fn count_gradual(e: &Expr) -> usize {
    let mut total = 0usize;
    count_gradual_recurse(e, &mut total);
    total
}

fn count_gradual_recurse(e: &Expr, total: &mut usize) {
    if matches!(&e.ty, Some(Ty::Untyped)) {
        *total += 1;
    }
    use ExprNode as N;
    match &*e.node {
        N::Lit { .. } | N::Var { .. } | N::Ivar { .. } | N::Const { .. } | N::SelfRef => {}
        N::If { cond, then_branch, else_branch } => {
            count_gradual_recurse(cond, total);
            count_gradual_recurse(then_branch, total);
            count_gradual_recurse(else_branch, total);
        }
        N::Send { recv, args, block, .. } => {
            if let Some(r) = recv { count_gradual_recurse(r, total); }
            for a in args { count_gradual_recurse(a, total); }
            if let Some(b) = block { count_gradual_recurse(b, total); }
        }
        N::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p { count_gradual_recurse(expr, total); }
            }
        }
        N::Seq { exprs } | N::Array { elements: exprs, .. } => {
            for x in exprs { count_gradual_recurse(x, total); }
        }
        N::BoolOp { left, right, .. } => {
            count_gradual_recurse(left, total);
            count_gradual_recurse(right, total);
        }
        N::RescueModifier { expr, fallback } => {
            count_gradual_recurse(expr, total);
            count_gradual_recurse(fallback, total);
        }
        N::Let { value, body, .. } => {
            count_gradual_recurse(value, total);
            count_gradual_recurse(body, total);
        }
        N::Lambda { body, .. } => count_gradual_recurse(body, total),
        N::Apply { fun, args, block } => {
            count_gradual_recurse(fun, total);
            for a in args { count_gradual_recurse(a, total); }
            if let Some(b) = block { count_gradual_recurse(b, total); }
        }
        N::Hash { entries, .. } => {
            for (k, v) in entries {
                count_gradual_recurse(k, total);
                count_gradual_recurse(v, total);
            }
        }
        N::Case { scrutinee, arms } => {
            count_gradual_recurse(scrutinee, total);
            for arm in arms {
                if let Some(g) = &arm.guard { count_gradual_recurse(g, total); }
                count_gradual_recurse(&arm.body, total);
            }
        }
        N::Assign { value, .. } => count_gradual_recurse(value, total),
        N::Yield { args } => for a in args { count_gradual_recurse(a, total); },
        N::Raise { value } | N::Return { value } => count_gradual_recurse(value, total),
        N::Super { args } => {
            if let Some(args) = args {
                for a in args { count_gradual_recurse(a, total); }
            }
        }
        N::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            count_gradual_recurse(body, total);
            for r in rescues {
                for c in &r.classes { count_gradual_recurse(c, total); }
                count_gradual_recurse(&r.body, total);
            }
            if let Some(eb) = else_branch { count_gradual_recurse(eb, total); }
            if let Some(en) = ensure { count_gradual_recurse(en, total); }
        }
        N::Next { value } => if let Some(v) = value { count_gradual_recurse(v, total); },
        N::MultiAssign { value, .. } => count_gradual_recurse(value, total),
        N::While { cond, body, .. } => {
            count_gradual_recurse(cond, total);
            count_gradual_recurse(body, total);
        }
        N::Range { begin, end, .. } => {
            if let Some(b) = begin { count_gradual_recurse(b, total); }
            if let Some(eb) = end { count_gradual_recurse(eb, total); }
        }
    }
}

fn collect_untyped(e: &Expr, path: &str, out: &mut Vec<String>) {
    let ty_ok = matches!(&e.ty, Some(t) if !matches!(t, Ty::Var { .. }));
    if !ty_ok {
        out.push(format!("{path}: {:?} has ty={:?}", &e.node, e.ty));
    }
    match &*e.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_untyped(cond, &format!("{path}/if.cond"), out);
            collect_untyped(then_branch, &format!("{path}/if.then"), out);
            collect_untyped(else_branch, &format!("{path}/if.else"), out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_untyped(r, &format!("{path}/send.recv"), out);
            }
            for (i, a) in args.iter().enumerate() {
                collect_untyped(a, &format!("{path}/send.arg[{i}]"), out);
            }
            if let Some(b) = block {
                collect_untyped(b, &format!("{path}/send.block"), out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for (i, p) in parts.iter().enumerate() {
                if let InterpPart::Expr { expr } = p {
                    collect_untyped(expr, &format!("{path}/interp[{i}]"), out);
                }
            }
        }
        ExprNode::Seq { exprs } => {
            for (i, e) in exprs.iter().enumerate() {
                collect_untyped(e, &format!("{path}/seq[{i}]"), out);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_untyped(left, &format!("{path}/boolop.left"), out);
            collect_untyped(right, &format!("{path}/boolop.right"), out);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            collect_untyped(expr, &format!("{path}/rescue.expr"), out);
            collect_untyped(fallback, &format!("{path}/rescue.fallback"), out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_untyped(value, &format!("{path}/let.value"), out);
            collect_untyped(body, &format!("{path}/let.body"), out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_untyped(body, &format!("{path}/lambda.body"), out)
        }
        ExprNode::Apply { fun, args, block } => {
            collect_untyped(fun, &format!("{path}/apply.fun"), out);
            for (i, a) in args.iter().enumerate() {
                collect_untyped(a, &format!("{path}/apply.arg[{i}]"), out);
            }
            if let Some(b) = block {
                collect_untyped(b, &format!("{path}/apply.block"), out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (i, (k, v)) in entries.iter().enumerate() {
                collect_untyped(k, &format!("{path}/hash[{i}].key"), out);
                collect_untyped(v, &format!("{path}/hash[{i}].value"), out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for (i, el) in elements.iter().enumerate() {
                collect_untyped(el, &format!("{path}/array[{i}]"), out);
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_untyped(scrutinee, &format!("{path}/case.scrut"), out);
            for (i, arm) in arms.iter().enumerate() {
                if let Some(g) = &arm.guard {
                    collect_untyped(g, &format!("{path}/case.arm[{i}].guard"), out);
                }
                collect_untyped(&arm.body, &format!("{path}/case.arm[{i}].body"), out);
            }
        }
        ExprNode::Assign { value, .. } => {
            collect_untyped(value, &format!("{path}/assign.value"), out)
        }
        ExprNode::Yield { args } => {
            for (i, a) in args.iter().enumerate() {
                collect_untyped(a, &format!("{path}/yield.arg[{i}]"), out);
            }
        }
        ExprNode::Raise { value } => {
            collect_untyped(value, &format!("{path}/raise.value"), out)
        }
        ExprNode::Return { value } => {
            collect_untyped(value, &format!("{path}/return.value"), out)
        }
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for (i, a) in args.iter().enumerate() {
                    collect_untyped(a, &format!("{path}/super.arg[{i}]"), out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_untyped(body, &format!("{path}/begin.body"), out);
            for (i, r) in rescues.iter().enumerate() {
                for (j, c) in r.classes.iter().enumerate() {
                    collect_untyped(c, &format!("{path}/begin.rescue[{i}].class[{j}]"), out);
                }
                collect_untyped(&r.body, &format!("{path}/begin.rescue[{i}].body"), out);
            }
            if let Some(e) = else_branch {
                collect_untyped(e, &format!("{path}/begin.else"), out);
            }
            if let Some(e) = ensure {
                collect_untyped(e, &format!("{path}/begin.ensure"), out);
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value {
                collect_untyped(v, &format!("{path}/next.value"), out);
            }
        }
        ExprNode::MultiAssign { value, .. } => {
            collect_untyped(value, &format!("{path}/multi_assign.value"), out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_untyped(cond, &format!("{path}/while.cond"), out);
            collect_untyped(body, &format!("{path}/while.body"), out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_untyped(b, &format!("{path}/range.begin"), out);
            }
            if let Some(e) = end {
                collect_untyped(e, &format!("{path}/range.end"), out);
            }
        }
    }
}

/// Every method body across every `runtime/ruby/*.rb` must be fully
/// typed — no None, no `Ty::Var` sentinels. `Ty::Untyped`
/// (RBS-declared gradual escape) is *allowed*: this is Bar A
/// (gradual-typed cleanly), separate from Bar B (concretely typed,
/// required for Rust emission). Mirrors the Rails-side promise
/// enforced by `tests/real_blog.rs::type_analysis_coverage`. New
/// runtime files are picked up automatically.
///
/// **Active** as of 2026-04-28: residual driven from 104 → 0
/// across one session via the three-path approach (path 1: analyzer
/// extensions for block_params_for/Lambda/Next/Yield/Super/Range,
/// stdlib coverage, Hash/Array narrowing→Untyped, constant tracking;
/// path 2: 9 RBS sidecars + splat fix + abstract pragma; path 3:
/// validates_*_of rewrite from block-yield to positional value).
/// Holds the typed-runtime promise: every framework Ruby method
/// body types end-to-end with no Var residual.
///
/// `Ty::Untyped` (RBS-declared gradual escape) is allowed and
/// counts as fully-typed (Bar A semantics). Bar B (zero
/// `Untyped`) is a separate, stricter goal tracked by the
/// `inference_on_spinel_blog_runtime_with_rbs::untyped_subexpressions_with_rbs_baseline`
/// probe and the GradualUntyped diagnostic pipeline.
#[test]
fn every_runtime_method_body_is_fully_typed() {
    let stems = runtime_ruby_stems();
    assert!(!stems.is_empty(), "runtime/ruby/ should have at least one .rb file");

    // Phase 1: unified class registry from all .rbs files so
    // cross-class method dispatch resolves during body-typing (e.g.,
    // RecordInvalid#initialize calls `record.errors.join(...)`, which
    // requires Base#errors to be known).
    let mut class_registry: std::collections::HashMap<ClassId, ClassInfo> =
        std::collections::HashMap::new();
    // Per-class include lists, accumulated across all .rbs files.
    // Key: short class id (last segment); Value: list of short module
    // ids the class includes.
    let mut includes_by_class: std::collections::HashMap<ClassId, Vec<ClassId>> =
        std::collections::HashMap::new();
    let mut missing_rbs: Vec<String> = Vec::new();

    let short_id = |class_id: &ClassId| {
        let last = class_id
            .0
            .as_str()
            .rsplit("::")
            .next()
            .unwrap_or(class_id.0.as_str())
            .to_string();
        ClassId(roundhouse::ident::Symbol::new(&last))
    };

    for stem in &stems {
        let rbs_path = Path::new("runtime/ruby").join(format!("{stem}.rbs"));
        if !rbs_path.exists() {
            missing_rbs.push(stem.clone());
            continue;
        }
        let rbs = fs::read_to_string(&rbs_path)
            .unwrap_or_else(|_| panic!("read {rbs_path:?}"));
        let per_file = match parse_app_signatures(&rbs) {
            Ok(m) => m,
            Err(_) => continue, // surfaces in phase 2
        };
        for (class_id, methods) in per_file {
            // parse_app_signatures returns fully-qualified names
            // (`ActiveRecord::Broadcasts`), but the body-typer builds
            // `Ty::Class { id }` using just the last segment of a
            // Const path. Strip to the last segment so lookups match.
            let entry = class_registry.entry(short_id(&class_id)).or_default();
            for (name, ty) in methods {
                // The dispatch table's value is the call's *result* type,
                // not the method's signature object. parse_app_signatures
                // returns Ty::Fn { params, ret, .. }; unwrap to ret so a
                // bare-name call like `table_name` resolves to Ty::Str
                // (its return) rather than Ty::Fn (the function value).
                let ret_ty = match ty {
                    Ty::Fn { ret, .. } => *ret,
                    other => other,
                };
                entry.instance_methods.insert(name, ret_ty);
            }
        }

        // Capture the include relationships so we can flatten
        // included-module methods into the including class. Body
        // dispatch resolves via per-class instance_methods only;
        // without the merge, `record.errors` (record: Base) wouldn't
        // find `errors` declared on Validations.
        if let Ok(includes) = parse_app_includes(&rbs) {
            for (class_id, included) in includes {
                let short = short_id(&class_id);
                let included_short: Vec<ClassId> = included.iter().map(short_id).collect();
                includes_by_class
                    .entry(short)
                    .or_default()
                    .extend(included_short);
            }
        }
    }

    // Phase 1.5: flatten includes. For each class C with `include M`,
    // copy M's instance_methods into C (existing entries on C win;
    // include only fills gaps). One pass is sufficient for the
    // current corpus (no transitive include chains); a fixed-point
    // loop becomes warranted only if a future RBS chain demands it.
    for (class_id, includes) in &includes_by_class {
        for included in includes {
            // Clone just the instance_methods map (ClassInfo as a
            // whole isn't Clone, and we only need the methods here).
            let included_methods: Vec<(roundhouse::ident::Symbol, Ty)> =
                match class_registry.get(included) {
                    Some(info) => info
                        .instance_methods
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    None => continue,
                };
            let entry = class_registry.entry(class_id.clone()).or_default();
            for (name, ty) in included_methods {
                entry.instance_methods.entry(name).or_insert(ty);
            }
        }
    }

    // Phase 2: per-file type-checking of method bodies against the
    // shared registry. Accumulate all errors so a failing run
    // enumerates every gap in one pass.
    let mut parse_or_type_errors: Vec<String> = Vec::new();
    let mut all_untyped: Vec<String> = Vec::new();

    for stem in &stems {
        let ruby_path = Path::new("runtime/ruby").join(format!("{stem}.rb"));
        let rbs_path = Path::new("runtime/ruby").join(format!("{stem}.rbs"));

        let ruby = fs::read_to_string(&ruby_path)
            .unwrap_or_else(|_| panic!("read {ruby_path:?}"));

        if !rbs_path.exists() {
            continue;
        }

        let rbs = fs::read_to_string(&rbs_path)
            .unwrap_or_else(|_| panic!("read {rbs_path:?}"));

        let methods = match parse_methods_with_rbs_in_ctx(&ruby, &rbs, &class_registry) {
            Ok(m) => m,
            Err(e) => {
                parse_or_type_errors.push(format!("{stem}: {e}"));
                continue;
            }
        };

        for m in &methods {
            let path = format!("{stem}.rb::{}", m.name);
            collect_untyped(&m.body, &path, &mut all_untyped);
        }
    }

    let _ = parse_methods_with_rbs; // preserve re-export

    let mut report: Vec<String> = Vec::new();
    if !missing_rbs.is_empty() {
        report.push(format!(
            "{} .rb file(s) without a paired .rbs:\n  {}",
            missing_rbs.len(),
            missing_rbs.join("\n  ")
        ));
    }
    if !parse_or_type_errors.is_empty() {
        report.push(format!(
            "{} parse/type error(s):\n  {}",
            parse_or_type_errors.len(),
            parse_or_type_errors.join("\n  ")
        ));
    }
    if !all_untyped.is_empty() {
        report.push(format!(
            "{} untyped sub-expression(s):\n  {}",
            all_untyped.len(),
            all_untyped.join("\n  ")
        ));
    }

    assert!(report.is_empty(), "{}", report.join("\n\n"));
}

/// **Bar B tracker** — counts `Ty::Untyped` sub-expressions across
/// the framework runtime corpus. These pass the strict Bar A test
/// (`every_runtime_method_body_is_fully_typed`) but represent
/// RBS-declared gradual escapes that block strict-target emission
/// (Rust). The CEILING is a soft tracker; assertion fires only if
/// the count *exceeds* it (so closures lower the ceiling, never
/// raise it without a code change).
///
/// The gap between Bar A (zero Var, currently passing) and Bar B
/// (zero Untyped, currently CEILING) is the work remaining for
/// Rust-emit-readiness: each Untyped site is either Pattern A
/// (per-model specialization), Pattern B (interface declaration),
/// Pattern C (narrowing inference), or Pattern D (block generics) —
/// per `project_ty_untyped_target_dependent`.
#[test]
fn every_runtime_method_body_concretely_typed() {
    let stems = runtime_ruby_stems();
    let mut class_registry: std::collections::HashMap<ClassId, ClassInfo> =
        std::collections::HashMap::new();
    let mut includes_by_class: std::collections::HashMap<ClassId, Vec<ClassId>> =
        std::collections::HashMap::new();

    let short_id = |class_id: &ClassId| {
        let last = class_id
            .0
            .as_str()
            .rsplit("::")
            .next()
            .unwrap_or(class_id.0.as_str())
            .to_string();
        ClassId(roundhouse::ident::Symbol::new(&last))
    };

    for stem in &stems {
        let rbs_path = Path::new("runtime/ruby").join(format!("{stem}.rbs"));
        if !rbs_path.exists() { continue; }
        let rbs = fs::read_to_string(&rbs_path).unwrap();
        let Ok(per_file) = parse_app_signatures(&rbs) else { continue };
        for (class_id, methods) in per_file {
            let entry = class_registry.entry(short_id(&class_id)).or_default();
            for (name, ty) in methods {
                let ret_ty = match ty {
                    Ty::Fn { ret, .. } => *ret,
                    other => other,
                };
                entry.instance_methods.insert(name, ret_ty);
            }
        }
        if let Ok(includes) = parse_app_includes(&rbs) {
            for (class_id, included) in includes {
                let short = short_id(&class_id);
                let included_short: Vec<ClassId> = included.iter().map(short_id).collect();
                includes_by_class.entry(short).or_default().extend(included_short);
            }
        }
    }
    for (class_id, includes) in &includes_by_class {
        for included in includes {
            let included_methods: Vec<(roundhouse::ident::Symbol, Ty)> =
                match class_registry.get(included) {
                    Some(info) => info
                        .instance_methods
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    None => continue,
                };
            let entry = class_registry.entry(class_id.clone()).or_default();
            for (name, ty) in included_methods {
                entry.instance_methods.entry(name).or_insert(ty);
            }
        }
    }

    let mut total_gradual: usize = 0;
    let mut by_file: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for stem in &stems {
        let ruby_path = Path::new("runtime/ruby").join(format!("{stem}.rb"));
        let rbs_path = Path::new("runtime/ruby").join(format!("{stem}.rbs"));
        if !rbs_path.exists() { continue; }
        let ruby = fs::read_to_string(&ruby_path).unwrap();
        let rbs = fs::read_to_string(&rbs_path).unwrap();
        let Ok(methods) = parse_methods_with_rbs_in_ctx(&ruby, &rbs, &class_registry)
            else { continue };
        for m in &methods {
            let n = count_gradual(&m.body);
            if n > 0 {
                *by_file.entry(stem.clone()).or_insert(0) += n;
                total_gradual += n;
            }
        }
    }

    eprintln!(
        "framework runtime Bar B residual (Ty::Untyped sites): {total_gradual} \
         across {} files",
        by_file.len(),
    );
    let mut breakdown: Vec<(String, usize)> = by_file.into_iter().collect();
    breakdown.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (stem, count) in &breakdown {
        eprintln!("  {stem}.rb: {count}");
    }

    // Ceiling — soft tracker. Tighten as Pattern A/B/C/D closures
    // land. Failing low is good (lower the ceiling and record).
    const CEILING: usize = 200;
    assert!(
        total_gradual <= CEILING,
        "{total_gradual} Ty::Untyped sites exceeds ceiling of {CEILING}",
    );
}
