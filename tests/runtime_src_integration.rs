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
    // The CtrlWalker/per-artifact retirement (2026-08-19) deleted
    // hand-written runtime/python/view_helpers.py — the overlay
    // transpiles `runtime/ruby/action_view/view_helpers.rb` into
    // `app/v2/view_helpers.py` instead, so there's no hand-written
    // destination to validate against (mirrors the elixir/go
    // retirements below). The emit itself stays pinned: the emitted
    // function must remain well-formed.
    let emitted = roundhouse::emit::python::emit_method(&pluralize_method());
    assert!(
        emitted.contains("def pluralize(count: int, word: str) -> str:"),
        "got:\n{emitted}"
    );
}

#[test]
fn inflector_pluralize_lives_in_runtime_rust() {
    let emitted = roundhouse::emit::rust::emit_method(&pluralize_method());
    assert_emitted_lives_in(&emitted, "runtime/rust/view_helpers.rs");
}

// Phase D3 (2026-06-05) retired runtime/elixir/view_helpers.ex — the v2
// path transpiles `runtime/ruby/action_view/view_helpers.rb` into the emit
// output instead of hand-writing the Elixir file, so there's no
// hand-written destination to validate against (mirrors the go retirement
// below). `emit::elixir::emit_method` was removed with the v1 app shell.

// Phase 6 step 3 (2026-05-24) retired runtime/go/view_helpers.go —
// the v2 path transpiles `runtime/ruby/action_view/view_helpers.rb`
// into the emit output instead of hand-writing the Go file. The
// runtime-extraction `emit::go::emit_method` helper itself stays
// (used elsewhere), but no longer has a hand-written destination
// file to validate against.

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
        N::Lit { .. }
        | N::Var { .. }
        | N::Ivar { .. }
        | N::Const { .. }
        | N::Retry
        | N::Redo
        | N::SelfRef => {}
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
        N::Assign { value, .. } | N::OpAssign { value, .. } => count_gradual_recurse(value, total),
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
        N::Next { value } | N::Break { value } => {
            if let Some(v) = value { count_gradual_recurse(v, total); }
        }
        N::Splat { value } => count_gradual_recurse(value, total),
        N::MultiAssign { value, .. } => count_gradual_recurse(value, total),
        N::While { cond, body, .. } => {
            count_gradual_recurse(cond, total);
            count_gradual_recurse(body, total);
        }
        N::Range { begin, end, .. } => {
            if let Some(b) = begin { count_gradual_recurse(b, total); }
            if let Some(eb) = end { count_gradual_recurse(eb, total); }
        }
        N::Cast { value, .. } => count_gradual_recurse(value, total),
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
        | ExprNode::Retry
        | ExprNode::Redo
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
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
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
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                collect_untyped(v, &format!("{path}/next.value"), out);
            }
        }
        ExprNode::Splat { value } => {
            collect_untyped(value, &format!("{path}/splat.value"), out);
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
        ExprNode::Cast { value, .. } => {
            collect_untyped(value, &format!("{path}/cast.value"), out);
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
    // The Db primitive shim's contract — connection.rb calls
    // `Db.prepare`/`step?`/`column_*` directly (same pre-seed the
    // production pipelines get via `seed_well_known_classes` /
    // `insert_framework_stubs`).
    roundhouse::lower::view_to_library::insert_db_stub(&mut class_registry);
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
            // (`ActiveRecord::Broadcasts`); the body-typer's Const arm
            // also builds `Ty::Class { id }` with the full path. Key
            // the registry on the full path so lookups match. Also
            // alias under the last segment so source-level bare
            // refs (`Article.find(...)`) resolve through the same
            // entry — the Const arm produces a single-segment Ty
            // for those.
            let short = short_id(&class_id);
            let methods_vec: Vec<(roundhouse::ident::Symbol, Ty)> = methods
                .into_iter()
                .map(|(name, ty)| {
                    let ret_ty = match ty {
                        Ty::Fn { ret, .. } => *ret,
                        other => other,
                    };
                    (name, ret_ty)
                })
                .collect();
            for (name, ret_ty) in &methods_vec {
                class_registry
                    .entry(class_id.clone())
                    .or_default()
                    .instance_methods
                    .insert(name.clone(), ret_ty.clone());
                if short != class_id {
                    class_registry
                        .entry(short.clone())
                        .or_default()
                        .instance_methods
                        .insert(name.clone(), ret_ty.clone());
                }
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

    // Phase 1.6: re-mirror methods between fully-qualified entries
    // and their last-segment aliases. The include-flattening pass
    // above keys on `short_id` (parse_app_includes returns the
    // including class as fully-qualified but the included module
    // as bare), so the merged-in methods only land on the short
    // alias. Body-typer dispatches via full-path `Ty::Class { id }`
    // (the Const arm builds the joined path), so without this
    // re-mirror, the included methods are invisible at the actual
    // dispatch site.
    let class_keys: Vec<ClassId> = class_registry.keys().cloned().collect();
    for key in &class_keys {
        let raw = key.0.as_str();
        let last = raw.rsplit("::").next().unwrap_or(raw);
        if last == raw {
            continue;
        }
        let short = ClassId(roundhouse::ident::Symbol::new(last));
        let short_methods: Vec<(roundhouse::ident::Symbol, Ty)> =
            match class_registry.get(&short) {
                Some(info) => info
                    .instance_methods
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                None => continue,
            };
        let entry = class_registry.entry(key.clone()).or_default();
        for (name, ty) in short_methods {
            entry.instance_methods.entry(name).or_insert(ty);
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
    // 2026-05-11 Phase 2.5(b) follow-up: HWIA source deleted (no
    // live require sites), dropping its 26 untyped contributions;
    // ceiling lowered 210 → 180 to lock in the gain.
    // 2026-05-11 Jbuilder Phase 2: json_builder.rb's encode_value
    // dispatches on `untyped` by design (JSON entry point); +7
    // untyped sites at the chained `v.is_a?(...)` / `v.to_s` calls;
    // ceiling raised 180 → 190.
    // 2026-07-18 relation-type-plan R5: Relation gains the Rails
    // array-delegation surface (`to_ary`, block `filter`, `&`/`|`/`-`)
    // — record-typed like `to_a`/`+` (element type is the model,
    // `untyped` in the runtime RBS by the same convention); +5 sites;
    // ceiling raised 190 → 195.
    // 2026-07-21 saved-change tracking (ActiveModel::Dirty subset):
    // base.rb's `__track_saved_changes` diffs the subclass
    // `attributes` hash, whose values are `untyped` by declaration —
    // the before/after value reads are irreducibly untyped; +2 sites.
    // Same day: Relation gains block `all?` (element-typed `untyped`
    // by the R5 delegation convention) for the Relation-returning
    // `Base.where` fallback; +2 sites; ceiling raised 195 → 199.
    // 2026-07-22 request/session two-layer split: ActionDispatch::
    // Request#format= takes `untyped` (Rails accepts a Symbol, the
    // writer coerces to the canonical String) and the `env` compat
    // bag's values are `untyped` by declaration (scratch keys like
    // `exception_notifier.exception_data`); +3 sites; ceiling raised
    // 199 → 202.
    // 2026-07-23 Relation#column_predicate record/collection dispatch
    // arms (`where(comment: comments)` IN-of-records): hash values are
    // `untyped` by declaration and the arms inspect them (`x` in the
    // ids map, `x.id`, `val.id`) — irreducible at this seam; +3 sites;
    // ceiling raised 202 → 205.
    // 2026-07-24 ActiveRecord::ValueTooLong (apps construct it to
    // reject over-long input — lobsters' Keystore): same optional-
    // message `super(...)` shape RecordNotFound already carries, whose
    // default-argument site is untyped by the same convention; +1
    // site; ceiling raised 205 → 206.
    // 2026-07-25 ActionController::CookieJar (controller `cookies[:k]`
    // access, ruby-family reopen replacing the CRuby-only overlay so
    // spinel-on-lobsters can read cookies): `[]`/`[]=`/`delete` take an
    // `untyped` key — cookies are indexed with both Symbol constants and
    // String literals, normalized via `key.to_s` — so the three key
    // params are untyped by declaration; +3 sites; ceiling raised
    // 206 → 209.
    // 2026-07-25 Inflector.pluralize_word (ActiveSupport String#pluralize
    // (count) grounded off the built-in String for spinel): `count` is
    // untyped — Rails compares `count == 1` and call sites pass either an
    // Integer or a whole collection (`"category".pluralize(@categories)`);
    // +1 site; ceiling raised 209 → 210.
    // 2026-07-26 ActionController::CookieJar#each (+#to_h): lobsters'
    // `remove_unknown_cookies` iterates the jar, so the typed CookieJar
    // needs the whole-jar walk Rails' Enumerable CookieJar provides. The
    // `yield k, v` expression is untyped — verified irreducible: the site
    // stays Ty::Untyped whatever the RBS block return is declared to be
    // (`-> void` and `-> String` both count), exactly as the same
    // `x.each { |e| yield e }` shape does in Relation#each/#find_each;
    // #to_h adds none (`@inbound.merge(@out)` stays concretely typed,
    // which is why it isn't an empty-literal accumulator); +1 site;
    // ceiling raised 210 → 211.
    // 2026-07-27 Arel::Table/Attribute (off the CRuby-only overlay into
    // the shared file so spinel-on-lobsters can compile
    // `Tag.arel_table[:id]`): `attribute`/`project` take an untyped
    // column (call sites pass Symbols and the `Arel.star` String alike)
    // and `in`/`not_in` an untyped subquery (anything answering
    // `to_sql` — a SelectManager or a Relation); +6 sites; ceiling
    // raised 211 → 217.
    // 2026-07-27 ActiveSupport.blank?/present?/presence (the runtime
    // grounding `src/lower/blank.rs` sends a receiver to when no static
    // type can answer): `value` is untyped BY CONTRACT — the whole
    // point is a predicate that branches on a value nothing typed — and
    // it is read across three entry points; +8 sites; ceiling raised
    // 217 → 232.
    // 2026-07-30 Base.upsert/upsert_all + Relation#pick (lobsters'
    // Keystore reads and writes every counter through them): the row
    // values are untyped BY CONTRACT — an upsert row is a
    // `Hash[Symbol, untyped]` whose values reach `escape_value`, the
    // same seam `update_counters` and `build_where` already sit on —
    // and `unique_by`/`on_duplicate` are Rails-shaped options arriving
    // as a String, a Symbol, or an Array. `pick` adds the one site
    // `pluck` beside it already has (the projected column's value).
    // `Base.primary_key` adds none: it returns a literal. +7 sites;
    // ceiling raised 232 → 245.
    // 2026-08-09 SignedCookieJar (`cookies.signed`, campfire's session
    // token): the KEY of every jar method is untyped by the same
    // contract the plain jar already documents — controllers index with
    // Symbol constants and String literals both — and `value_of`'s
    // parameter is genuinely poly, since Rails takes either a bare
    // value or an options Hash whose values are String, bool and Symbol
    // together. Confining that read to one class method is what keeps
    // it off the jar's own String-typed surface. +10 sites; ceiling
    // raised 245 → 255.
    // 2026-08-11 ActiveRecord::RecordNotUnique (campfire's first-run
    // screen rescues it to turn a lost race into a redirect): the one
    // site is the `super(message)` call in its constructor, the same
    // shape `RecordNotFound` and `ValueTooLong` already contribute here
    // — `super` has no signature for the typer to resolve. +1 site;
    // ceiling raised 255 -> 256. Growth from one more class in the
    // corpus, not from typing getting worse.
    // 2026-08-16 `Relation#excluding` (campfire's messages#create skips
    // the poster when it fans a new message out to bot webhooks): the
    // two new sites are in relation.rb, the only body added, and both
    // are its `*records` splat — untyped BY CONTRACT for the same reason
    // `where`'s condition is. Rails' `excluding` takes a record, an
    // array of records, or a relation, and this method's whole job is to
    // hand that straight to `column_predicate`, which already dispatches
    // on all three; typing the parameter would mean picking one. +2
    // sites; ceiling raised 256 -> 258.
    // 2026-08-16 `Request.for` (the shared constructor the test harness
    // uses to build a request on either target): the one site is the
    // `env.each { |k, v| r.env[k] = v }` widening copy. `@env` is
    // `Hash[String, untyped]` BY CONTRACT — this file's own header says
    // callers write scratch keys of any type into it
    // (`exception_notifier.exception_data`) — while a caller's env
    // literal is `Hash[String, String]`, so the copy is what bridges the
    // two. Assigning instead of copying is a real type error that only a
    // strict target notices, and spinel named it exactly once matz's
    // `77cc33c9` began failing the build on wrong-typed pointers.
    // Typing the block params would mean claiming env values are
    // Strings, which is the thing that is false. +1 site; ceiling raised
    // 258 -> 259.
    // 2026-08-16 the cookie jar's WRITE side, wired up for campfire's
    // integration tests. Three sites, and each is a value this runtime
    // has no business narrowing:
    //   - `CookieJar#[]=` / `#raw_set`'s value (2). A cookie is a String
    //     on the wire and app code writes whatever it has —
    //     campfire's TrackedRoomVisit writes `@room.id`, an Integer.
    //     `raw_set` coerces, so the STORE stays String→String and the
    //     RETURN stays String; only the argument is untyped, which is
    //     the one thing that is actually true of it. Declaring `String`
    //     here (what it said before) was the lie that let an Integer
    //     into the map unremarked.
    //   - `ActionDispatch::Cookies::CookieJar.build`'s request (1).
    //     Rails threads a request through for the key generator and for
    //     host-scoping `domain: :all`; this runtime models neither, so
    //     the parameter exists to match the documented call shape and is
    //     never read. Typing it would claim it participates.
    // +3 sites; ceiling raised 259 -> 262.
    // 2026-08-16 `Relation#destroy_all` — Rails' callback-running
    // counterpart to `delete_all` (campfire prunes a user's search
    // history through it, and the `after_destroy` hooks are the whole
    // point). Its three sites are the `Array[untyped]` return and the
    // per-record reads in `records.each { |r| r.destroy }`. The element
    // type is untyped because this Relation is NOT generic over its
    // model — `to_a`, `first` and `find` beside it say the same thing,
    // and narrowing one of them means narrowing all of them. Rails'
    // return value (the destroyed records) is kept rather than swapped
    // for a count: `delete_all` already answers a count, and having the
    // two differ only in callbacks is the distinction worth preserving.
    // +3 sites; ceiling raised 262 -> 265.
    // 2026-08-17 the Enumerable surface campfire's suite reaches for —
    // `partition`, `detect`, `sort_by`, `without` (Rails' own alias for
    // `excluding`), and the block form of `select`. Every one is
    // `to_a.<m> { |x| yield x }`, and every new site is the
    // `Array[untyped]` element `to_a` hands back plus the block's
    // parameter read: exactly what `map`, `group_by`, `find_each` and
    // `destroy_all` already contribute, and untyped for the one reason
    // they all share — this Relation is not generic over its model. A
    // generic `Relation[T]` retires the class of them at once; adding
    // Enumerable methods one at a time neither helps nor hurts that.
    // +8 sites; ceiling raised 265 -> 273.
    // 2026-08-17 `Relation#destroy_by` / `#delete_by` — Rails'
    // condition-taking bulk writes, the pair that stands to
    // `destroy_all`/`delete_all` as `find_by` stands to `find`. Each
    // contributes its `conditions` parameter (an untyped condition hash,
    // exactly as `where`'s already is beside it) and the untyped result
    // it forwards from the terminal it delegates to. Neither body does
    // anything but `where(conditions).<terminal>`; nothing that was
    // concrete became gradual. campfire's `Room has_many :memberships do
    // def revoke_from(users) destroy_by user: users end end` is the
    // caller that wanted them.
    // +2 sites; ceiling raised 273 -> 275.
    // 2026-08-18 `active_record/signed_id.rb` — ONE site for the whole
    // file: `Time.now + expires_in`. `Time#+` is deliberately
    // `Ty::Untyped` in `time_method` (the receiver-only dispatch cannot
    // tell a Duration arg, which gives a Time, from a Time arg, which
    // gives a Float), and this is the ordinary consumer of that. The
    // result feeds `iso8601_ms`, whose `::Time` parameter absorbs it.
    // Its neighbour `action_controller/message_verifier.rb` gained
    // three methods in the same change and contributes nothing here.
    // +1 site; ceiling raised 275 -> 276.
    // 2026-08-18 `Request.for`'s `params` copy — the sibling of the `env`
    // copy two entries above, and untyped for the same declared reason
    // (`@params` is `Hash[String, untyped]`). It stopped being an
    // assignment because a bare `{}` default is Symbol-keyed on a strict
    // target, so the block's value read is the one new site. Same change
    // put `.to_s` on the seven `env[...]` reads, which coerce a declared
    // `untyped` into the String the attribute holds and add nothing here.
    // +1 site; ceiling raised 276 -> 277.
    // 2026-08-18 `Relation#scoped_write_where` — the WHERE a bulk write
    // takes, an `IN (SELECT …)` subquery when the scope carries a JOIN
    // (SQL gives a DELETE nowhere to put one). Two sites, both rooted in
    // `@model`: the constructor's parameter is untyped by declaration,
    // so `@model.primary_key` is a gradual send and the `key` local it
    // binds inherits that. Exactly the shape `@wheres`/`@joins` reads
    // already contribute across this file; the two callers
    // (`delete_all`, `update_all`) each lost a line and gained none.
    // +2 sites; ceiling raised 277 -> 279.
    // 2026-08-18 `Relation#join_fragment` — the guard that makes
    // `joins(:assoc)` RAISE rather than append the bare symbol as SQL,
    // where SQLite reads it as a table alias and answers the wrong rows
    // instead of failing. Two sites, both on `spec`, whose `untyped` is
    // declared by `joins` itself: the guarded `return spec` and the
    // interpolation in the raise. The check is a type test on a value
    // whose type the caller decides, which is what this file's other
    // `untyped`-parameter sites all are.
    // +2 sites; ceiling raised 279 -> 281.
    // 2026-08-19 `Params.require_key` — Rails' `params.require(:url)`,
    // which asserts a parameter was supplied and answers its value.
    // Two sites, both on the value read out of the params tree: it is a
    // `Roundhouse::ParamValue` (the String | Hash | Array union), so
    // the `fetch` and the value it binds are gradual by declaration —
    // the same shape every other reader in params.rb contributes.
    // +2 sites; ceiling raised 281 -> 283.
    // 2026-08-19 the ActiveModel::Dirty VALUE half —
    // `attribute_previously_was` on both layers plus `_note_hydrated`.
    // Four sites, all on the `name` parameter (a caller-chosen Symbol)
    // and the diff's heterogeneous values, which is what every other
    // entry in params.rb and this file's Dirty surface already
    // contributes. The method exists twice because the strict lanes
    // cannot index the `[prev, value]` pair, so each layer pays its own.
    // +2 sites; ceiling raised 283 -> 285.
    // 2026-08-20 285 -> 291, five methods the campfire suite named, an
    // average of two sites each:
    //   relation.rb +2. `collect` is `map`'s second name and pays
    //     exactly what `map` pays — the element type is `untyped` in
    //     the runtime RBS by the R5 delegation convention, so the block
    //     param and the `yield` are both untyped. (A `Relation#new`
    //     added 6 more here and was reverted: spinel already names that
    //     class's constructor `sp_Relation_new`.)
    //   flash.rb +4. `mark_shown` and `FlashNow`'s `[]`/`[]=` take an
    //     untyped key for the same reason CookieJar's do — flash is
    //     indexed with Symbol constants and String literals alike, and
    //     the body normalizes via `key.to_s`.
    //   base.rb +0. `destroy!` calls `destroy`, which is typed `Base`.
    // The STI recast contributes nothing: its column copy lives in
    // src/lower/sti_scope.rs, not here — see the note on the other
    // ceiling in tests/inference_on_spinel_blog_runtime_with_rbs.rs for
    // why that placement was not optional.
    // 2026-08-20 291 -> 293: `Relation#first` and `#find` restoring the
    // state they set (the terminal invariant — see the note on the
    // other ceiling). One site each, and zero tests moved.
    // 2026-08-21 293 -> 298: `ViewHelpers.mail_to`. FIVE sites, one per
    // mail header lifted out of the html options — each is an
    // `opts.fetch(:cc, nil)` bound to a local, the same shape (and the
    // same reason) as `button_to`'s `opts.fetch(:method, nil)` beside
    // it: the opts hash is `Hash[Symbol, untyped]` by declaration, so
    // every read out of it is gradual. They are five rather than one
    // because the list is UNROLLED — a `next` inside an `each` over a
    // constant list is not a shape the Rust emitter lowers. Everything
    // downstream is typed: `mail_query_append` takes three Strings, so
    // the gradual value never crosses a call boundary.
    // 2026-08-21 298 -> 308: `Relation#find_or_create_by`, the same ten
    // sites the other ceiling itemizes (it counts eight; this counter
    // also charges the two `nil?`/`save` sends on the untyped locals).
    // Reads off `conditions` (`Hash[Symbol, untyped]`), off `@model`,
    // and off what `find_by`/`new` answer — the four sources every
    // relation method in this file already draws on. campfire's
    // `Search.record` is `find_or_create_by(query: query).touch`
    // reached through `user.searches`, and until this existed the
    // emitted body called a method nothing defined.
    // 2026-08-21 308 -> 312: `Relation#==`, four sites where the other
    // ceiling counts twelve (that one charges receivers as well). The
    // four are the `other` parameter, the `theirs` local it flows into,
    // and the `.id` read on an element of each side — `Array[untyped]`
    // by the same R5 delegation convention every method in this file
    // pays. Nothing gradual crosses a call boundary: what leaves is a
    // `bool`.
    //
    // See the note on the other ceiling for why the id comparison is
    // HERE rather than in a `Base#==`: an operator definition in
    // base.rb reaches every strict target and no emitter renames one
    // (python emitted `def ==(self, other)` and every tree stopped at a
    // SyntaxError), while the ruby-family reopen in connection.rb has
    // no assignment to type `@id` from.
    // 2026-08-21 312 -> 315: `ActionText::Fragment`, three sites. Two
    // are what the `replace` block answers — a filter returns markup,
    // a sanitizer returns nil, and `.to_s` is what reconciles them,
    // which is Rails' own contract for that block. The third is
    // `Fragment.wrap`'s parameter, whose whole job is to accept either
    // a Fragment or a String. Everything the scanner itself touches is
    // typed: `Node` carries three declared fields and `Selector` six,
    // classes rather than hashes precisely so no bag appears here.
    // 2026-08-22 315 -> 316: `Relation#find` raising `RecordNotFound`
    // on no match, which is Rails' whole distinction between it and
    // `find_by` — and what turns a missing record into a 404 rather
    // than a nil that NoMethodErrors somewhere later. ONE site: the
    // `record.nil?` guard on what the terminal answered, which is
    // `untyped` by the same R5 delegation convention every terminal in
    // this file pays (`find_by!` two lines down has the identical
    // guard and the identical site). Nothing new became gradual — the
    // method already read that local to return it.
    // 2026-08-23 316 -> 317: `SignedId.verified_id!`, the raising twin
    // of the sentinel read — so `find_signed!` answers
    // `InvalidSignature` for a token that does not verify and
    // `RecordNotFound` only for one that does and names no row. ONE
    // site: `ActiveRecord::SignedId.verified_id(...)` is a class-method
    // send, and this gate's registry is built from the runtime's own
    // `.rbs` INSTANCE methods, so its result is `untyped` here however
    // the sidecar declares it. The comparison against it is the whole
    // method.
    // 2026-08-23 317 -> 319: the three relation TERMINALS that borrow
    // relation state and now give it back — `find_by` and `exists?(id)`
    // pop the predicate they pushed, `first_n` restores the limit it
    // borrowed, the same discipline `find` and `pick` already spell
    // out one screen up. TWO sites, both the locals the restore
    // forces: `find_by`'s `record` and `exists?`'s `found` hold the
    // answer across the pop, and each is the R5 `untyped` every
    // terminal in this file already pays. Not optional bookkeeping:
    // campfire's `find_messages` probes `find_by(id: params[
    // :message_id])` and then pages THE SAME relation, so on a plain
    // /rooms/1 the leftover `WHERE id IS NULL` made a room of a
    // hundred messages render zero — behind a 200 and a well-formed
    // page, which is why no test in the app saw it.
    const CEILING: usize = 319;
    assert!(
        total_gradual <= CEILING,
        "{total_gradual} Ty::Untyped sites exceeds ceiling of {CEILING}",
    );
}
