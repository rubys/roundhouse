//! Inference probe: how far does roundhouse's whole-program type
//! inference reach on framework-runtime code with NO RBS?
//!
//! The spinel-blog fixture is a metaprogramming-free Ruby blog whose
//! `runtime/ruby/active_record/*.rb` files are hand-shaped specimens of
//! framework code that fits Spinel's documented subset (no eval, no
//! send, no method_missing). They ship without RBS sidecars on
//! purpose: spinel uses pure inference. Running the same files
//! through roundhouse's analyzer with no RBS measures how close our
//! inference gets to spinel's coverage on the same input.
//!
//! This is the diagnostic that justifies the three-gap inference
//! work (return propagation, parameter unification, fixpoint). The
//! count reported here is the residual — what inference still can't
//! reach. If the residual is small enough, the framework-typing
//! prerequisite for Rust no longer requires authoring RBS for
//! runtime files; targeted RBS for public library signatures
//! suffices.
//!
//! Distinct from `runtime_src_integration::every_runtime_method_body_is_fully_typed`,
//! which gates on RBS-paired typing (different path, different
//! contract) and is `#[ignore]`'d while the framework-typing residual
//! closes. This test runs by default and tracks the inference-only
//! number — same fixture surface, different machinery.

use std::fs;
use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::expr::{Expr, ExprNode, InterpPart};
use roundhouse::ingest::ingest_library_classes;
use roundhouse::ty::Ty;
use roundhouse::App;

const RUNTIME_DIR: &str = "runtime/ruby/active_record";

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

fn build_app_from_runtime() -> App {
    let mut app = App::new();
    let dir = Path::new(RUNTIME_DIR);
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {RUNTIME_DIR}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rb"))
        .collect();
    entries.sort();
    for path in entries {
        let source = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let path_str = path.display().to_string();
        let classes = ingest_library_classes(&source, &path_str)
            .unwrap_or_else(|e| panic!("ingest {path_str}: {e:?}"));
        for lc in classes {
            app.library_classes.push(lc);
        }
    }
    app
}

/// Diagnostic probe — runs the analyzer (with the new fixpoint
/// loop) over the spinel-blog runtime files and reports the residual
/// untyped count. Asserts only that the count doesn't regress past a
/// known ceiling, so the test fails loud if a future change breaks
/// inference; tighten the bound as inference improves. The actual
/// count is printed with `--nocapture` so progress is visible.
#[test]
fn untyped_subexpressions_baseline() {
    let mut app = build_app_from_runtime();
    Analyzer::new(&app).analyze(&mut app);

    let mut all_untyped: Vec<String> = Vec::new();
    for lc in &app.library_classes {
        for method in &lc.methods {
            let path = format!("{}#{}", lc.name.0.as_str(), method.name.as_str());
            collect_untyped(&method.body, &path, &mut all_untyped);
        }
    }

    eprintln!(
        "spinel-blog runtime active_record/: {} untyped sub-expressions \
         across {} library classes",
        all_untyped.len(),
        app.library_classes.len()
    );

    // Loose ceiling — current measurement plus headroom. Tighten as
    // inference improves; failing low is a good thing (un-pin and
    // record the new lower bound). The point of the bound is to
    // catch regressions, not to lock in today's number.
    const CEILING: usize = 500;
    assert!(
        all_untyped.len() <= CEILING,
        "{} untyped sub-expressions on spinel-blog runtime — exceeds ceiling of {CEILING}.\n\
         First 20:\n  {}",
        all_untyped.len(),
        all_untyped.iter().take(20).cloned().collect::<Vec<_>>().join("\n  ")
    );
}
