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
use std::process::Command;

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

#[test]
fn active_support_squish_bang_emits_for_rust() {
    let methods = load_typed("active_support_ext");
    let method = methods
        .into_iter()
        .find(|m| m.name.as_str() == "squish!")
        .expect("active_support_ext.rb defines squish!");
    let emitted = roundhouse::emit::rust::emit_method(&method);
    assert!(
        emitted.starts_with("pub fn squish_bang(text: &mut String) -> &mut String"),
        "ActiveSupport.squish! must emit a mutating Rust function:\n{emitted}"
    );

    let scratch = std::env::temp_dir().join(format!(
        "roundhouse-runtime-rust-squish-bang-{}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch).expect("create generated Rust scratch directory");
    let source = format!(
        r#"{emitted}
#[test]
fn squish_bang_mutates_and_returns_the_receiver() {{
    let mut value = "  foo\tbar \n baz  ".to_string();
    let receiver = &value as *const String;
    let result = squish_bang(&mut value);
    assert_eq!(result, "foo bar baz");
    assert_eq!(result as *const String, receiver);
    assert_eq!(value, "foo bar baz");
}}

#[test]
fn squish_bang_handles_unicode_and_noop_values() {{
    let mut unicode = "\u{{00a0}}foo\u{{2003}}bar\u{{2028}}".to_string();
    let receiver = &unicode as *const String;
    let result = squish_bang(&mut unicode);
    assert_eq!(result, "foo bar");
    assert_eq!(result as *const String, receiver);
    assert_eq!(unicode, "foo bar");
    let mut unchanged = "already squished".to_string();
    let receiver = &unchanged as *const String;
    let result = squish_bang(&mut unchanged);
    assert_eq!(result, "already squished");
    assert_eq!(result as *const String, receiver);
    assert_eq!(unchanged, "already squished");
}}

#[test]
fn squish_bang_strips_rails_nul_boundaries() {{
    let mut trailing_nul = "foo\0".to_string();
    let receiver = &trailing_nul as *const String;
    let result = squish_bang(&mut trailing_nul);
    assert_eq!(result, "foo");
    assert_eq!(result as *const String, receiver);
    assert_eq!(trailing_nul, "foo");

    let mut space_then_nul = "foo \0".to_string();
    let receiver = &space_then_nul as *const String;
    let result = squish_bang(&mut space_then_nul);
    assert_eq!(result, "foo");
    assert_eq!(result as *const String, receiver);
    assert_eq!(space_then_nul, "foo");
}}
"#
    );
    let source_path = scratch.join("squish_bang.rs");
    let binary_path = scratch.join("squish_bang");
    fs::write(&source_path, source).expect("write emitted Rust test");
    let compile = Command::new("rustc")
        .args(["--test"])
        .arg(&source_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("compile emitted Rust test");
    assert!(
        compile.status.success(),
        "emitted ActiveSupport.squish! did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&binary_path)
        .output()
        .expect("run emitted Rust test");
    assert!(
        run.status.success(),
        "emitted ActiveSupport.squish! behavior failed:\n{}",
        String::from_utf8_lossy(&run.stdout)
    );
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
        | N::ForwardArgs
        | N::ForwardKeywords
        | N::Defined { .. }
        | N::SelfRef => {}
        N::ForwardKeywordsWithPairs { entries } => {
            for (key, value) in entries {
                count_gradual_recurse(key, total);
                count_gradual_recurse(value, total);
            }
        }
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
        N::MethodRef { recv, .. } => {
            if let Some(r) = recv {
                count_gradual_recurse(r, total);
            }
        }
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
        N::CaseMatch { scrutinee, arms, else_body } => {
            count_gradual_recurse(scrutinee, total);
            for arm in arms {
                arm.pattern.for_each_expr(&mut |e| count_gradual_recurse(e, total));
                if let Some((_, g)) = &arm.guard { count_gradual_recurse(g, total); }
                count_gradual_recurse(&arm.body, total);
            }
            if let Some(e) = else_body { count_gradual_recurse(e, total); }
        }
        N::MatchPredicate { value, pattern } | N::MatchRequired { value, pattern } => {
            count_gradual_recurse(value, total);
            pattern.for_each_expr(&mut |e| count_gradual_recurse(e, total));
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
        N::Splat { value } | N::KeywordSplat { value } => count_gradual_recurse(value, total),
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
        | ExprNode::ForwardArgs
        | ExprNode::ForwardKeywords
        | ExprNode::Defined { .. }
        | ExprNode::SelfRef => {}
        ExprNode::ForwardKeywordsWithPairs { entries } => {
            for (key, value) in entries {
                collect_untyped(key, path, out);
                collect_untyped(value, path, out);
            }
        }
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
        ExprNode::MethodRef { recv, .. } => {
            if let Some(r) = recv {
                collect_untyped(r, &format!("{path}/method_ref.recv"), out);
            }
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
        ExprNode::CaseMatch { scrutinee, arms, else_body } => {
            collect_untyped(scrutinee, &format!("{path}/case_match.scrut"), out);
            for (i, arm) in arms.iter().enumerate() {
                arm.pattern.for_each_expr(&mut |e| {
                    collect_untyped(e, &format!("{path}/case_match.arm[{i}].pattern"), out);
                });
                if let Some((_, g)) = &arm.guard {
                    collect_untyped(g, &format!("{path}/case_match.arm[{i}].guard"), out);
                }
                collect_untyped(&arm.body, &format!("{path}/case_match.arm[{i}].body"), out);
            }
            if let Some(e) = else_body {
                collect_untyped(e, &format!("{path}/case_match.else"), out);
            }
        }
        ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
            collect_untyped(value, &format!("{path}/match.value"), out);
            pattern.for_each_expr(&mut |e| {
                collect_untyped(e, &format!("{path}/match.pattern"), out);
            });
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
        ExprNode::Splat { value } | ExprNode::KeywordSplat { value } => {
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
    // …and the stdlib surface, which the app path gets from
    // `Analyzer::new` and this registry did not. The runtime files are
    // re-analyzed by that path once they land in an emitted tree, so a
    // registry poorer than it is what makes this gate report a gap the
    // real pipeline does not have: `rescue => e; e.message` types here
    // as unknown and there as String. Entries are `or_insert`, so an
    // `.rbs` in runtime/ruby always wins over a stdlib default.
    roundhouse::analyze::register_stdlib_classes(&mut class_registry);
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

    // Soft Bar B ratchet: fails when residual rises. Tighten after a
    // measured drop; never raise without a ledgered feature. Residual
    // still dominated by polymorphic SQL / helper-opt hashes;
    // `Relation[T]` is the longer-term fix. Merged main's 299; measure
    // after the security helpers before changing this number.
    // `ActionController::RoutingError#initialize` adds 2: its
    // `super(message)` is gradual, as in `ParameterMissing`.
    // `Timeout.timeout` (Spinel port for Campfire tip) adds 3: Pattern D
    // block/return gradual after `sec: Integer | Float` — polymorphic yield.
    // `AttachedMany#attachments` stays typed via raw SQL + ManyAttachment
    // (not Relation over the synthesized Attachment MODEL).
    // `ActiveRecord::Base#with_lock` (#644) adds 1: its block's return
    // value is gradual, same `untyped` escape as `self.transaction`'s
    // block value (`lock!` itself stays concretely typed — it's a
    // plain `reload`).
    // `with_lock`'s Rails 8.1 options split (#671) adds 5: the trailing
    // transaction-options Hash this runtime doesn't carry
    // `extract_options!` for is read by hand (`transaction_opts[:iso
    // lation]`, `[:requires_new]`, `.key?(:joinable) ? [:joinable] :
    // true`), and each `Hash[Symbol, untyped]` value read is gradual —
    // same shape `connection.rb`'s other `opts`-style Hash call sites
    // already carry.
    // `ActiveRecord::RecordNotFound#initialize` (model/primary_key/id)
    // adds 1, MEASURED on the runtime after #671: the `@id = id` store
    // of the id Rails passes through, whose RBS type is the flat
    // `String | Integer | Float | Array | nil` union (Float added after
    // #689 review; MEASURED, no change).
    // Campfire's repin past 2393f01 adds 5, MEASURED, each from a value
    // Rails itself leaves dynamic: `Connection#select_value` (one SQL cell, as
    // `select_rows`' rows are), `Base.uncached`'s block value (the
    // `Timeout.timeout` shape), and `Relation#to_h`'s yielded pairs.
    // `ActiveStorage::AttachedMany#each` (Rails' `delegate_missing_to
    // :attachments`, for campfire's `body.embeds.each`) adds 1,
    // MEASURED: the value of its `yield`, the same block-return escape
    // `Relation#each` carries.
    // `in_batches` adds 2, MEASURED: the `yield self` in
    // `Relation#in_batches` and in the class-side fallback in
    // connection.rb, whose value is the block's — gradual, as
    // `find_in_batches`' `yield records` already is.
    // `Relation#minimum` / `#maximum` add 2, MEASURED: the two indexed
    // reads of the SQL aggregate's column-dependent scalar. The adapter
    // contract intentionally keeps raw SQL result values `untyped`;
    // coercing them would break Rails' column-dependent return type.
    // MEASURED 2026-10-09 (against origin/main a28539b6): `haml_class`
    // (the HAML shortcut-class + hash `class:` merge) adds 2 on top of
    // the above — its `value` param is `untyped` (a scalar the HAML
    // compiler could not narrow further: String, Symbol, nil, or a
    // conditional/ternary result), and the body reads it twice
    // (`value.nil?`, `value.to_s`).
    // Rebased onto main after `in_batches`: MEASURED 317 with haml_class, under main's 318.
    // `self.transaction`'s `ensure`-based depth/commit restore on a
    // non-local exit (spinel-txn-pin #693, CodeRabbit): the nested
    // branch's `ensure` (a second `Db._txn_depth = depth`, alongside the
    // `rescue`'s own copy, now folded into the `begin`'s value position)
    // adds 1; the outer branch's `ensure` adds 3 — its `if rolled_back
    // then nil else … end` has to unify `nil` against `Db.exec("COMMIT")`'s
    // declared `void` return, the same kind of escape this file's other
    // `opts`-style branches already carry. Rebased onto main's
    // `in_batches` (#713): re-measured fresh on this tree rather than
    // summed from either side's base, since unrelated main changes
    // shift base.rb/connection.rb's own counts independently
    // (spinel-txn-pin #693) — 321, MEASURED after rebasing past #705/#709.
    // `Relation#minimum` / `#maximum` add 2, MEASURED: the two indexed
    // reads of the SQL aggregate's column-dependent scalar. The adapter
    // contract intentionally keeps raw SQL result values `untyped`;
    // coercing them would break Rails' column-dependent return type. The
    // merged tree measures 323 after the transaction-runtime change above.
    // Schema-driven extrema deserialization adds 9 measured gradual sites
    // in Relation: aggregate values and group keys cross the raw SQL boundary
    // as column-dependent Boolean/Date/Time values. The concrete method-body
    // gate still requires every call and body to resolve.
    // Decimal extrema normalization adds 2 measured gradual sites: the raw
    // adapter value is column-dependent, and its `to_f` conversion is the
    // schema-selected Ruby boundary that matches the model's Float contract.
    // The emitted regression covers scalar and grouped decimal extrema.
    // Canonical main ae7bf6cf measures 334 sites; #720's Ruby runtime
    // additions contribute 39 more, primarily from generic JSON, deep_dup,
    // session and request-environment values. Keep those intentional dynamic
    // boundaries visible in the measured combined ceiling.
    // `transaction(requires_new: true)` savepoints add 3 in
    // `self.transaction`, MEASURED 373 -> 376 on main 9d577c24: the
    // `requires_new` option is `untyped` (as `with_lock` forwards it), and
    // the nested block's value is now read back after its RELEASE.
    const CEILING: usize = 376;
    assert!(
        total_gradual <= CEILING,
        "{total_gradual} Ty::Untyped sites exceeds ceiling of {CEILING}",
    );
}

#[test]
fn active_support_inflector_slice_adds_no_bar_b_sites() {
    let methods = load_typed("active_support_inflections");
    for name in [
        "camelize",
        "deconstantize",
        "foreign_key",
        "upcase_first",
        "downcase_first",
    ] {
        let method = methods
            .iter()
            .find(|method| {
                method.name.as_str() == name
                    && method
                        .enclosing_class
                        .as_ref()
                        .is_some_and(|class| class.as_str() == "Inflector")
            })
            .unwrap_or_else(|| panic!("ActiveSupport::Inflector.{name} exists in runtime source"));
        assert_eq!(
            count_gradual(&method.body),
            0,
            "ActiveSupport::Inflector.{name} introduces no Ty::Untyped Bar B sites",
        );
    }
}

#[test]
fn empty_html_opts_emits_string_keyed_maps_on_csharp_and_kotlin() {
    let src = include_str!("../runtime/ruby/action_view/view_helpers.rb");
    let consts = roundhouse::runtime_src::parse_module_constant_exprs(src).unwrap();
    let empty = consts
        .iter()
        .find(|(n, _)| n.as_str() == "EMPTY_HTML_OPTS")
        .expect("EMPTY_HTML_OPTS");
    let cs = roundhouse::emit::csharp::emit_module_constant(empty.0.as_str(), &empty.1);
    assert!(
        cs.contains("Dictionary<string, object?> EMPTY_HTML_OPTS"),
        "empty frozen opts constant must not degrade to object?: {cs}"
    );
    let kt = roundhouse::emit::kotlin::emit_constant_for_runtime(&empty.1);
    assert_eq!(kt, "mutableMapOf<String, Any?>()");

    // Explicit `{}` typed Hash[untyped, untyped] must stay String-keyed.
    use roundhouse::expr::{Expr, ExprNode};
    use roundhouse::span::Span;
    use roundhouse::ty::Ty;
    let mut arg = Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries: vec![], kwargs: false },
    );
    arg.ty = Some(Ty::Hash {
        key: Box::new(Ty::Untyped),
        value: Box::new(Ty::Untyped),
    });
    let emitted = roundhouse::emit::kotlin::emit_expr_for_runtime(&arg);
    assert_eq!(
        emitted, "mutableMapOf<String, Any?>()",
        "empty untyped hash arg must pin String keys for Kotlin invariance"
    );
}
