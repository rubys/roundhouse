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
use roundhouse::rbs::{parse_app_ivars, parse_app_signatures};
use roundhouse::ty::Ty;

const RUNTIME_DIR: &str = "runtime/ruby/active_record";

/// Sidecars outside `active_record/` that AR bodies call. Without these
/// the probe reports Ty::Var on real, already-declared surfaces (Db,
/// Rails.application, MessageVerifier, ActiveSupport.json_time) — the
/// same gap `runtime_src_integration` avoids via `insert_db_stub` /
/// stdlib seeding. Not an app inventory; shared-runtime contracts only.
const DEPENDENCY_RBS: &[&str] = &[
    "runtime/ruby/db.rbs",
    "runtime/ruby/rails.rbs",
    "runtime/ruby/action_controller/message_verifier.rbs",
    "runtime/ruby/active_support_time_parsing.rbs",
    "runtime/ruby/active_support_ext.rbs",
    // `Inflector.pluralize_word`, for the multi-id RecordNotFound message.
    "runtime/ruby/inflector_ext.rbs",
];

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

/// Seed ivar_bindings: union-merge flow assignments (so `initialize`'s
/// `@limit = nil` does not permanently beat `limit`'s `@limit = n`),
/// then overlay RBS-declared ivars for this class (single source of
/// truth — no parallel class-name match table).
///
/// `ActiveRecord::Base` methods are split across stems (`base.rb`,
/// `connection.rb`, …). Callers must merge seeds for the same
/// `ClassId` — a per-stem insert would drop `initialize`'s writes.
fn seed_ivars_for_class(
    class_id: &ClassId,
    methods: &[roundhouse::dialect::MethodDef],
    rbs_ivars: &HashMap<ClassId, HashMap<Symbol, Ty>>,
) -> HashMap<Symbol, Ty> {
    let mut flow: HashMap<Symbol, Ty> = HashMap::new();
    for method in methods {
        roundhouse::analyze::extract_ivar_assignments(&method.body, &mut flow);
    }
    if let Some(declared) = rbs_ivars.get(class_id) {
        for (name, ty) in declared {
            flow.insert(name.clone(), ty.clone());
        }
    }
    // Short-name RBS keys (rare) — only when exact ClassId missed.
    // Do not merge short-only ivars onto a class that already has its
    // own declaration map (would widen unrelated siblings).
    if !rbs_ivars.contains_key(class_id) {
        if let Some(short) = class_id.0.as_str().rsplit("::").next() {
            let short_id = ClassId(Symbol::new(short));
            if short_id != *class_id {
                if let Some(declared) = rbs_ivars.get(&short_id) {
                    for (name, ty) in declared {
                        flow.entry(name.clone()).or_insert_with(|| ty.clone());
                    }
                }
            }
        }
    }
    flow
        .into_iter()
        .map(|(name, ty)| {
            // Already nilable overlays stay; flow Int becomes Int|Nil.
            let wrapped = match &ty {
                Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => ty,
                other => Ty::Union {
                    variants: vec![other.clone(), Ty::Nil].into(),
                },
            };
            (name, wrapped)
        })
        .collect()
}

/// Union-merge two ivar maps (same join as the analyzer harvest).
fn merge_ivar_maps(into: &mut HashMap<Symbol, Ty>, from: HashMap<Symbol, Ty>) {
    for (name, ty) in from {
        match into.remove(&name) {
            Some(prev) => {
                into.insert(name, match (prev, ty) {
                    (a, b) if a == b => a,
                    (a, b) => {
                        // Flatten nested unions so `Array[Int]|Nil` ⨝ `Nil`
                        // stays a flat union (collection_elem still sees Int).
                        let mut variants = Vec::new();
                        let mut push = |t: Ty| match t {
                            Ty::Union { variants: inner } => {
                                for v in inner {
                                    if !variants.iter().any(|e| e == &v) {
                                        variants.push(v);
                                    }
                                }
                            }
                            other => {
                                if !variants.iter().any(|e| e == &other) {
                                    variants.push(other);
                                }
                            }
                        };
                        push(a);
                        push(b);
                        match variants.len() {
                            0 => Ty::Nil,
                            1 => variants.pop().unwrap(),
                            _ => Ty::Union { variants: variants.into() },
                        }
                    }
                });
            }
            None => {
                into.insert(name, ty);
            }
        }
    }
}

/// Preserve canonical RBS class identities for method parameters and
/// dispatch. This probe has no ConstantResolver, so unique short-name
/// aliases also serve bare constants. Ambiguous suffixes must never
/// merge unrelated classes.
///
/// Also merges dependency sidecars + the Db stub the production /
/// Bar-A paths already seed, so AR bodies are not scored as unresolved
/// for calls that are already contracted elsewhere in `runtime/ruby/`.
fn build_class_registry() -> (
    HashMap<ClassId, ClassInfo>,
    HashMap<ClassId, HashMap<Symbol, Ty>>,
    HashMap<ClassId, HashMap<Symbol, Ty>>,
) {
    let dir = Path::new(RUNTIME_DIR);
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {RUNTIME_DIR}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rbs"))
        .collect();
    entries.sort();
    for dep in DEPENDENCY_RBS {
        entries.push(Path::new(dep).to_path_buf());
    }

    let mut sigs: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
    let mut ivars: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
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
        let by_ivar = parse_app_ivars(&source).unwrap_or_else(|e| {
            panic!("parse ivars {}: {e}", path.display())
        });
        for (class_id, class_ivars) in by_ivar {
            ivars.entry(class_id).or_default().extend(class_ivars);
        }
    }
    let mut registry = registry_with_unique_aliases(&sigs);
    // Db lives in class_methods on the production stub; mirror that so
    // `Db.exec` / `Db.escape_string` resolve the same way as Bar A.
    roundhouse::lower::view_to_library::insert_db_stub(&mut registry);
    (registry, sigs, ivars)
}

/// Build exact entries first, then alias only unambiguous short references.
/// Keep full Fn signatures so dispatch and body contexts see the same RBS.
/// Mirror each sig into `class_methods` too: `parse_app_signatures` does
/// not split `def self`, and Const receivers consult class_methods first.
fn registry_with_unique_aliases(
    sigs: &HashMap<ClassId, HashMap<Symbol, Ty>>,
) -> HashMap<ClassId, ClassInfo> {
    let mut registry: HashMap<ClassId, ClassInfo> = HashMap::new();
    let mut aliases: HashMap<ClassId, Option<ClassId>> = HashMap::new();
    for (class_id, methods) in sigs {
        let entry = registry.entry(class_id.clone()).or_default();
        entry.instance_methods = methods.clone();
        entry.class_methods = methods.clone();
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
                info.class_methods = sigs[&owner].clone();
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
    ctx.self_ty = Some(Ty::Class { id: class_id.clone(), args: vec![].into() });
    ctx.ivar_bindings = ivars.clone();
    if let Some(class_sigs) = sigs.get(class_id) {
        if let Some(Ty::Fn { params, .. }) = class_sigs.get(&method.name) {
            for (param, p) in method.params.iter().zip(params.iter()) {
                ctx.local_bindings.insert(param.name.clone(), (*p.ty).clone());
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
    let (registry, sigs, _) = build_class_registry();
    let classes = ingest_runtime_classes();
    let row = Ty::Hash {
        key: std::sync::Arc::new(Ty::Str),
        value: std::sync::Arc::new(Ty::Untyped),
    };
    let rows = Ty::Array { elem: std::sync::Arc::new(row) };
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
                args: vec![].into(),
            },
        );
        typer.analyze_expr(&mut probes[0].methods[0].body, &ctx);
        // `rows` answers Rails' shape, each row an Array of its values;
        // the constructor above still takes the row hashes.
        assert_eq!(
            probes[0].methods[0].body.ty,
            Some(Ty::Array { elem: std::sync::Arc::new(Ty::Array { elem: std::sync::Arc::new(Ty::Untyped) }) }),
            "{class}"
        );
    }
}

/// Parameter seeds must reach the actual nested classes, and a canonical
/// parser result must retain its typed validity/value readers on dispatch.
#[test]
fn qualified_finder_signatures_seed_contexts_and_results() {
    let (registry, sigs, _) = build_class_registry();
    let classes = ingest_runtime_classes();
    let key_class = ClassId(Symbol::new("ActiveRecord::IntegerKeyCast"));
    let input = Ty::Union { variants: vec![Ty::Int, Ty::Float, Ty::Str, Ty::Nil].into() };
    for (class, method, expected) in [
        ("ActiveRecord::IntegerKeyCast", "initialize", vec![("valid", Ty::Bool), ("value", Ty::Int)]),
        ("ActiveRecord::IntegerKeyCast", "parse", vec![("id", input.clone())]),
        ("ActiveRecord::Base", "find", vec![("id", input)]),
    ] {
        let (_, lc) = classes.iter().find(|(_, lc)| lc.name.0.as_str() == class &&
            lc.methods.iter().any(|m| m.name.as_str() == method)).expect("runtime class/method");
        let method = lc.methods.iter().find(|m| m.name.as_str() == method).unwrap();
        let ctx = build_method_ctx(&lc.name, method, &sigs, &HashMap::new());
        for (name, ty) in expected {
            assert_eq!(ctx.local_bindings.get(&Symbol::new(name)), Some(&ty), "{class}.{name}");
        }
    }
    let Ty::Fn { ret, .. } = &sigs[&key_class][&Symbol::new("parse")] else { panic!("parse signature"); };
    assert_eq!(ret.as_ref(), &Ty::Class { id: key_class.clone(), args: vec![].into() });
    let source = b"class Probe; def valid(cast); cast.valid; end; def value(cast); cast.value; end; end";
    let mut probes = ingest_library_classes(source, "probe.rb").unwrap();
    let mut ctx = Ctx::default();
    ctx.local_bindings.insert(Symbol::new("cast"), std::sync::Arc::unwrap_or_clone(ret.clone()));
    let typer = BodyTyper::new(&registry);
    for method in &mut probes[0].methods {
        typer.analyze_expr(&mut method.body, &ctx);
        let expected = if method.name.as_str() == "valid" { Ty::Bool } else { Ty::Int };
        assert_eq!(method.body.ty, Some(expected));
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
    let (registry, sigs, rbs_ivars) = build_class_registry();
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

    // Harvest ivars per ClassId across every stem (Base spans base.rb
    // + connection.rb). Union-merge so a later stem cannot wipe
    // initialize's writes. Overlay from parsed RBS `@ivar` decls.
    let mut ivars_by_class: HashMap<ClassId, HashMap<Symbol, Ty>> = HashMap::new();
    for (_, lc) in &classes {
        let seeded = seed_ivars_for_class(&lc.name, &lc.methods, &rbs_ivars);
        merge_ivar_maps(ivars_by_class.entry(lc.name.clone()).or_default(), seeded);
    }

    for (_, lc) in &mut classes {
        let lc_name = lc.name.clone();
        let wrapped = ivars_by_class.get(&lc_name).cloned().unwrap_or_default();
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

    // Soft ratchet under canonical class-ID lookup (Fixes #435), then
    // dependency RBS + cross-stem ivar merge + Relation/Base overlays.
    // Fails only when the residual rises. Not a substitute for Bar B.
    const CEILING: usize = 0;

    assert!(
        all_untyped.len() <= CEILING,
        "{} untyped sub-expressions exceeds ceiling of {CEILING}.\n\
         First 30:\n  {}",
        all_untyped.len(),
        all_untyped.iter().take(30).cloned().collect::<Vec<_>>().join("\n  ")
    );
}
