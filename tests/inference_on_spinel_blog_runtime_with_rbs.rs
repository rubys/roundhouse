//! RBS-paired probe: how far does typing reach on spinel-blog
//! runtime when paired with hand-authored RBS sidecars?
//!
//! Same fixture surface as `tests/inference_on_spinel_blog_runtime.rs`
//! (the no-RBS probe), but consults the `.rbs` files alongside the
//! `.rb` files in `runtime/ruby/active_record/`.
//! The residual count is the empirical answer to "would narrow RBS
//! close the framework-typing gap for strict targets?"
//!
//! Distinct from `runtime_src_integration::every_runtime_method_body_is_fully_typed`,
//! which sweeps the same framework Ruby (now at `runtime/ruby/*`) but
//! enforces strict zero-untyped — currently `#[ignore]`'d while the
//! residual closes. This test probes the same metaprogramming-free
//! corpus and tracks the residual count via CEILING; it's the project
//! tracker that demonstrates progress as the strict bar approaches.
//!
//! Test structure mirrors the no-RBS probe (registry build, per-file
//! body-typing, walk untyped expressions) but seeds method-body Ctx
//! with parameter types from the matching RBS signature, which the
//! analyzer's library_class path doesn't do today (`build_method_ctx`
//! below is the parse_methods_with_rbs_in_ctx flow recreated for the
//! library_class shape).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use roundhouse::analyze::{BodyTyper, ClassInfo, Ctx};
use roundhouse::dialect::{LibraryClass, MethodDef};
use roundhouse::expr::{Expr, ExprNode, InterpPart};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_library_classes;
use roundhouse::rbs::parse_app_signatures;
use roundhouse::ty::Ty;

const RUNTIME_DIR: &str = "runtime/ruby/active_record";

/// Walk a typed expression tree, collecting every node whose `ty` is
/// missing or `Ty::Var`. Same shape as the no-RBS probe so the two
/// numbers are directly comparable.
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

/// Walk one method body collecting every `@x = expr` assignment so
/// the second typing pass can seed `ivar_bindings` with discovered
/// types. Mirrors the analyzer's two-pass discipline (model + library
/// passes).
fn extract_ivar_assignments(expr: &Expr, out: &mut HashMap<Symbol, Ty>) {
    match &*expr.node {
        ExprNode::Assign {
            target: roundhouse::expr::LValue::Ivar { name },
            value,
        } => {
            if let Some(ty) = value.ty.clone() {
                out.entry(name.clone()).or_insert(ty);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                extract_ivar_assignments(e, out);
            }
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            extract_ivar_assignments(then_branch, out);
            extract_ivar_assignments(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            extract_ivar_assignments(left, out);
            extract_ivar_assignments(right, out);
        }
        ExprNode::Lambda { body, .. } => extract_ivar_assignments(body, out),
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            extract_ivar_assignments(body, out);
            for r in rescues {
                extract_ivar_assignments(&r.body, out);
            }
            if let Some(e) = else_branch {
                extract_ivar_assignments(e, out);
            }
            if let Some(e) = ensure {
                extract_ivar_assignments(e, out);
            }
        }
        ExprNode::Return { value } => extract_ivar_assignments(value, out),
        _ => {}
    }
}

/// Preserve canonical RBS class identities for method parameters and
/// dispatch. This probe has no ConstantResolver, so unique short-name
/// aliases also serve bare constants. Ambiguous suffixes must never
/// merge unrelated classes.
fn build_class_registry() -> (HashMap<ClassId, ClassInfo>, HashMap<ClassId, HashMap<Symbol, Ty>>) {
    let dir = Path::new(RUNTIME_DIR);
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {RUNTIME_DIR}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rbs"))
        .collect();
    entries.sort();

    let mut sigs: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
    for path in entries {
        let source = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("read {}: {e}", path.display())
        });
        let by_class = parse_app_signatures(&source).unwrap_or_else(|e| {
            panic!("parse {}: {e}", path.display())
        });
        for (class_id, methods) in by_class {
            // Sorted sidecars retain the existing reopen/override order.
            sigs.entry(class_id).or_default().extend(methods);
        }
    }
    (registry_with_unique_aliases(&sigs), sigs)
}

/// Build exact entries first, then alias only unambiguous short references.
/// Keep full Fn signatures so dispatch and body contexts see the same RBS.
fn registry_with_unique_aliases(
    sigs: &HashMap<ClassId, HashMap<Symbol, Ty>>,
) -> HashMap<ClassId, ClassInfo> {
    let mut registry: HashMap<ClassId, ClassInfo> = HashMap::new();
    let mut aliases: HashMap<ClassId, Option<ClassId>> = HashMap::new();
    for (class_id, methods) in sigs {
        registry.entry(class_id.clone()).or_default().instance_methods = methods.clone();
        let short = ClassId(Symbol::new(
            class_id.0.as_str().rsplit("::").next().unwrap_or(class_id.0.as_str()),
        ));
        aliases
            .entry(short)
            .and_modify(|owner| {
                if owner.as_ref() != Some(class_id) {
                    *owner = None;
                }
            })
            .or_insert_with(|| Some(class_id.clone()));
    }
    for (short, owner) in aliases {
        if let Some(owner) = owner {
            registry.entry(short).or_insert_with(|| {
                let mut info = ClassInfo::default();
                info.instance_methods = sigs[&owner].clone();
                info
            });
        }
    }
    registry
}

/// Build a method-body Ctx by seeding `self_ty` from the enclosing
/// class and `local_bindings` from the matching RBS signature's
/// params (positional zip). If no RBS signature matches the method
/// name, locals stay empty — body-typer falls back to `Ty::Var`
/// for `Var { name }` reads.
fn build_method_ctx(
    class_id: &ClassId,
    method: &MethodDef,
    sigs: &HashMap<ClassId, HashMap<Symbol, Ty>>,
    ivars: &HashMap<Symbol, Ty>,
) -> Ctx {
    let mut ctx = Ctx::default();
    ctx.self_ty = Some(Ty::Class { id: class_id.clone(), args: vec![] });
    ctx.ivar_bindings = ivars.clone();
    if let Some(class_sigs) = sigs.get(class_id) {
        if let Some(Ty::Fn { params, .. }) = class_sigs.get(&method.name) {
            for (param, p) in method.params.iter().zip(params.iter()) {
                ctx.local_bindings.insert(param.name.clone(), p.ty.clone());
            }
        }
    }
    ctx
}

fn ingest_runtime_classes() -> Vec<(String, LibraryClass)> {
    let dir = Path::new(RUNTIME_DIR);
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {RUNTIME_DIR}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rb"))
        .collect();
    entries.sort();
    let mut out = Vec::new();
    for path in entries {
        let source = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let path_str = path.display().to_string();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let classes = ingest_library_classes(&source, &path_str)
            .unwrap_or_else(|e| panic!("ingest {path_str}: {e:?}"));
        for lc in classes {
            out.push((stem.clone(), lc));
        }
    }
    out
}

/// Existing nested runtime classes must receive their declared parameter
/// types, and fully-qualified receiver types must resolve their readers.
#[test]
fn qualified_runtime_signatures_seed_contexts_and_results() {
    let (registry, sigs) = build_class_registry();
    let classes = ingest_runtime_classes();
    let row = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Untyped),
    };
    let rows = Ty::Array { elem: Box::new(row) };
    for (class, method, name, expected) in [
        ("ActiveRecord::Result", "initialize", "rows", rows.clone()),
        ("ActiveRecord::Connection", "quote_string", "str", Ty::Str),
    ] {
        let (_, lc) = classes
            .iter()
            .find(|(_, lc)| lc.name.0.as_str() == class)
            .expect("runtime class");
        let method = lc.methods.iter().find(|m| m.name.as_str() == method).unwrap();
        let ctx = build_method_ctx(&lc.name, method, &sigs, &HashMap::new());
        assert_eq!(
            ctx.local_bindings.get(&Symbol::new(name)),
            Some(&expected),
            "{class}.{name}"
        );
    }
    let source = b"class Probe; def rows(result); result.rows; end; end";
    let typer = BodyTyper::new(&registry);
    for class in ["ActiveRecord::Result", "Result"] {
        // Each spelling starts untyped so prior annotations cannot mask a miss.
        let mut probes = ingest_library_classes(source, "probe.rb").unwrap();
        let mut ctx = Ctx::default();
        ctx.local_bindings.insert(
            Symbol::new("result"),
            Ty::Class {
                id: ClassId(Symbol::new(class)),
                args: vec![],
            },
        );
        typer.analyze_expr(&mut probes[0].methods[0].body, &ctx);
        assert_eq!(
            probes[0].methods[0].body.ty,
            Some(rows.clone()),
            "{class}"
        );
    }
}

/// A short alias may not combine identically named classes in distinct modules.
#[test]
fn ambiguous_short_aliases_do_not_merge_signatures() {
    let signatures = parse_app_signatures(
        "module One\n class Shared\n def first: () -> Integer\n end\nend\n\
         module Two\n class Shared\n def second: () -> String\n end\nend\n",
    )
    .unwrap();
    let registry = registry_with_unique_aliases(&signatures);
    assert!(!registry.contains_key(&ClassId(Symbol::new("Shared"))));
    let one = &registry[&ClassId(Symbol::new("One::Shared"))].instance_methods;
    let two = &registry[&ClassId(Symbol::new("Two::Shared"))].instance_methods;
    assert!(one.contains_key(&Symbol::new("first")) && !one.contains_key(&Symbol::new("second")));
    assert!(two.contains_key(&Symbol::new("second")) && !two.contains_key(&Symbol::new("first")));
}

#[test]
fn untyped_subexpressions_with_rbs_baseline() {
    let (registry, sigs) = build_class_registry();
    let mut classes = ingest_runtime_classes();
    let typer = BodyTyper::new(&registry);

    // Pass 1: type each method body once with empty ivar_bindings.
    for (_, lc) in &mut classes {
        let lc_name = lc.name.clone();
        for method in &mut lc.methods {
            let empty: HashMap<Symbol, Ty> = HashMap::new();
            let ctx = build_method_ctx(&lc_name, method, &sigs, &empty);
            typer.analyze_expr(&mut method.body, &ctx);
        }
    }

    // Harvest ivar assignments per class. Then re-type with the
    // discovered ivars (each wrapped in `Union<T, Nil>` to reflect
    // a possible pre-write nil read) seeded into Ctx.
    let mut ivars_by_class: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
    for (_, lc) in &classes {
        let entry = ivars_by_class.entry(lc.name.clone()).or_default();
        for method in &lc.methods {
            extract_ivar_assignments(&method.body, entry);
        }
    }

    for (_, lc) in &mut classes {
        let lc_name = lc.name.clone();
        let mut wrapped: HashMap<Symbol, Ty> = HashMap::new();
        if let Some(found) = ivars_by_class.get(&lc_name) {
            for (k, v) in found {
                wrapped.insert(k.clone(), Ty::Union { variants: vec![v.clone(), Ty::Nil] });
            }
        }
        for method in &mut lc.methods {
            let ctx = build_method_ctx(&lc_name, method, &sigs, &wrapped);
            typer.analyze_expr(&mut method.body, &ctx);
        }
    }

    // Walk every method body, collect untyped sub-expressions, and
    // tally per-file (per-source .rb) plus an overall total.
    let mut by_file: HashMap<String, usize> = HashMap::new();
    let mut all_untyped: Vec<String> = Vec::new();
    for (stem, lc) in &classes {
        for method in &lc.methods {
            let path = format!("{}::{}#{}", stem, lc.name.0.as_str(), method.name.as_str());
            let before = all_untyped.len();
            collect_untyped(&method.body, &path, &mut all_untyped);
            let added = all_untyped.len() - before;
            *by_file.entry(stem.clone()).or_insert(0) += added;
        }
    }

    let mut breakdown: Vec<(String, usize)> = by_file.into_iter().collect();
    breakdown.sort_by_key(|(k, _)| k.clone());
    eprintln!(
        "spinel-blog runtime active_record/ (with hand-authored RBS): \
         {} untyped sub-expressions across {} library classes",
        all_untyped.len(),
        classes.len()
    );
    for (stem, count) in &breakdown {
        eprintln!("  {stem}.rb: {count}");
    }

    // Dump residual list when RUNTIME_PROBE_DUMP=1 so the human can
    // inspect what's left to categorize. Skip in normal runs to keep
    // the output tidy.
    if std::env::var("RUNTIME_PROBE_DUMP").is_ok() {
        for line in &all_untyped {
            eprintln!("{line}");
        }
    }

    // Soft ratchet under canonical class-ID lookup (Fixes #435). Fails
    // only when the residual rises. Growth here is mostly Relation SQL
    // composers (self-sends and query ivars), not a substitute for Bar B.
    const CEILING: usize = 57;

    assert!(
        all_untyped.len() <= CEILING,
        "{} untyped sub-expressions exceeds ceiling of {CEILING}.\n\
         First 30:\n  {}",
        all_untyped.len(),
        all_untyped.iter().take(30).cloned().collect::<Vec<_>>().join("\n  ")
    );
}
