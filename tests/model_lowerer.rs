//! Step 3 — first session: model lowerers from Rails-shape `Model` to
//! the universal post-lowering `LibraryClass` whose body is a flat
//! sequence of `MethodDef`s. The forcing function is the spinel-blog
//! fixture pair: real-blog/app/models/article.rb (Rails DSL) lowers to
//! a LibraryClass structurally matching spinel-blog/app/models/article.rb
//! (explicit method bodies).
//!
//! Comparison is structural at the IR level — method names, parameter
//! lists, receiver kinds. Body shapes are spot-checked rather than
//! deep-compared because the spinel-blog fixture is hand-written and
//! carries stylistic choices (variable naming, formatting) that the
//! lowerer's output won't match byte-for-byte. See the handoff for the
//! "structural compare passes ≠ textual match required" calibration.

use std::path::Path;

use roundhouse::dialect::{LibraryClass, MethodReceiver};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app;
use roundhouse::lower::{
    class_info_from_library_class, lower_controller_to_library_class,
    lower_controllers_to_library_classes, lower_model_to_library_class,
    lower_models_to_library_classes, lower_models_with_registry,
    lower_test_modules_to_library_classes, lower_view_to_library_class,
    lower_views_to_library_classes,
};

fn fixture_path() -> &'static Path {
    Path::new("fixtures/real-blog")
}

fn lower(name: &str) -> LibraryClass {
    let app = ingest_app(fixture_path()).expect("ingest real-blog");
    let model = app
        .models
        .iter()
        .find(|m| m.name.0.as_str() == name)
        .unwrap_or_else(|| panic!("model {name} not in real-blog"));
    lower_model_to_library_class(model, &app.schema)
}

fn method_names(lc: &LibraryClass) -> Vec<&str> {
    lc.methods.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn application_record_lowers_with_abstract_marker() {
    // application_record.rb is abstract — no schema table, no
    // associations, no validations. The `primary_abstract_class`
    // marker lowers to `def self.abstract?; true; end`; nothing else
    // synthesizes.
    let lc = lower("ApplicationRecord");
    assert_eq!(lc.name.0.as_str(), "ApplicationRecord");
    let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
    assert_eq!(parent, "ActiveRecord::Base", "parent: {parent}");
    assert!(!lc.is_module);
    assert_eq!(method_names(&lc), vec!["abstract?"]);
    let m = &lc.methods[0];
    assert!(matches!(m.receiver, MethodReceiver::Class));
    assert!(m.params.is_empty());
}

#[test]
fn article_lowers_with_schema_methods() {
    let lc = lower("Article");
    assert_eq!(lc.name.0.as_str(), "Article");
    let parent = lc.parent.as_ref().map(|p| p.0.as_str()).unwrap_or("(none)");
    assert_eq!(parent, "ApplicationRecord");

    let names = method_names(&lc);

    // Per-column accessors (excluding id — inherits from base).
    for col in ["title", "body", "created_at", "updated_at"] {
        assert!(names.contains(&col), "missing reader `{col}`: {names:?}");
        let writer = format!("{col}=");
        assert!(
            names.iter().any(|n| *n == writer.as_str()),
            "missing writer `{writer}`: {names:?}",
        );
    }
    // id reader/writer ARE synthesized (per-class so target emitters
    // can declare `id: number` as a typed field on the subclass; the
    // ApplicationRecord baseline registration doesn't surface a
    // declaration on Article in the lowered IR). Earlier shape skipped
    // id with the rationale "ApplicationRecord owns it"; that worked
    // for typer dispatch but left TS without a field declaration.
    assert!(
        names.contains(&"id"),
        "id reader should be synthesized: {names:?}",
    );
    assert!(
        names.contains(&"id="),
        "id writer should be synthesized: {names:?}",
    );

    // The non-attr scaffold: table_name, schema_columns, instantiate,
    // initialize, attributes, [], []=, update.
    for expected in [
        "table_name",
        "schema_columns",
        "instantiate",
        "initialize",
        "attributes",
        "[]",
        "[]=",
        "update",
    ] {
        assert!(
            names.contains(&expected),
            "missing scaffold method `{expected}`: {names:?}",
        );
    }

    // Receiver checks: table_name, schema_columns, instantiate are class
    // methods; everything else is instance.
    let class_methods = ["table_name", "schema_columns", "instantiate"];
    for m in &lc.methods {
        let n = m.name.as_str();
        if class_methods.contains(&n) {
            assert!(
                matches!(m.receiver, MethodReceiver::Class),
                "`{n}` should be a class method, got {:?}",
                m.receiver,
            );
        } else {
            assert!(
                matches!(m.receiver, MethodReceiver::Instance),
                "`{n}` should be an instance method, got {:?}",
                m.receiver,
            );
        }
    }
}

#[test]
fn article_lowers_has_many_to_collection_reader() {
    let lc = lower("Article");
    let comments = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "comments")
        .expect("comments method present (has_many :comments)");

    assert!(matches!(comments.receiver, MethodReceiver::Instance));
    assert!(comments.params.is_empty());

    // Body should be `Comment.where(article_id: @id)`.
    let (recv_path, method) = match &*comments.body.node {
        roundhouse::ExprNode::Send { recv, method, .. } => {
            let recv = recv.as_ref().expect("comments body should be Comment.where(...)");
            let path = match &*recv.node {
                roundhouse::ExprNode::Const { path } => {
                    path.iter().map(|s| s.as_str().to_string()).collect::<Vec<_>>()
                }
                other => panic!("comments receiver should be Const; got {other:?}"),
            };
            (path, method.as_str().to_string())
        }
        other => panic!("comments body is not Send: {other:?}"),
    };
    assert_eq!(recv_path, vec!["Comment".to_string()]);
    assert_eq!(method, "where");
}

#[test]
fn article_lowers_validate_method() {
    let lc = lower("Article");
    let validate = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "validate")
        .expect("validate method present (article has presence/length validations)");

    assert!(matches!(validate.receiver, MethodReceiver::Instance));
    assert!(validate.params.is_empty());

    // Body is a Seq of one Send per (attr, rule) pair. Article has:
    //   validates :title, presence: true              → 1 call
    //   validates :body,  presence: true, length: {…} → 2 calls
    let body = &*validate.body.node;
    let exprs = match body {
        roundhouse::ExprNode::Seq { exprs } => exprs,
        other => panic!("validate body is not Seq: {other:?}"),
    };
    assert!(
        exprs.len() >= 3,
        "expected >=3 validates_* calls (presence on title, presence+length on body); got {}: {exprs:?}",
        exprs.len(),
    );

    // Each call passes the value as a positional `@attr` Ivar arg
    // (no block). Spot-check the first.
    let first = exprs.first().unwrap();
    let (method_name, args, block) = match &*first.node {
        roundhouse::ExprNode::Send { method, args, block, .. } => {
            (method.as_str(), args, block)
        }
        other => panic!("first validate stmt is not Send: {other:?}"),
    };
    assert!(
        method_name.starts_with("validates_"),
        "first stmt should be a validates_* helper; got {method_name}",
    );
    assert!(block.is_none(), "validates_* helper should not carry a block");
    assert!(args.len() >= 2, "expected >=2 args (attr + value); got {}", args.len());
    // Second positional arg is the @attr Ivar.
    match &*args[1].node {
        roundhouse::ExprNode::Ivar { .. } => {}
        other => panic!(
            "validates_* second arg should be `@attr` (Ivar); got {other:?}",
        ),
    }
}

#[test]
fn comment_lowers_validate_with_two_presence_calls() {
    let lc = lower("Comment");
    let validate = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "validate")
        .expect("validate method present");

    let exprs = match &*validate.body.node {
        roundhouse::ExprNode::Seq { exprs } => exprs.clone(),
        other => panic!("validate body should be Seq; got {other:?}"),
    };
    assert_eq!(
        exprs.len(),
        2,
        "Comment has presence on commenter + body → 2 calls; got {}",
        exprs.len(),
    );
    for e in &exprs {
        match &*e.node {
            roundhouse::ExprNode::Send { method, .. } => {
                assert_eq!(method.as_str(), "validates_presence_of");
            }
            other => panic!("validate stmt should be Send; got {other:?}"),
        }
    }
}

#[test]
fn comment_lowers_belongs_to_reader() {
    let lc = lower("Comment");
    assert_eq!(lc.name.0.as_str(), "Comment");

    let article = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "article")
        .expect("article method present (belongs_to :article)");

    assert!(matches!(article.receiver, MethodReceiver::Instance));
    assert!(article.params.is_empty());

    // Shape: `if @article_id == 0 then nil else Article.find_by(id: @article_id) end`.
    match &*article.body.node {
        roundhouse::ExprNode::If { cond, .. } => match &*cond.node {
            roundhouse::ExprNode::Send { method, .. } => {
                assert_eq!(method.as_str(), "==", "guard should be ==");
            }
            other => panic!("if-cond should be Send `==`; got {other:?}"),
        },
        other => panic!("article body should be If; got {other:?}"),
    }
}

#[test]
fn article_lowers_dependent_destroy_to_before_destroy() {
    let lc = lower("Article");
    let cb = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "before_destroy")
        .expect("before_destroy method present (has_many dependent: :destroy)");

    assert!(matches!(cb.receiver, MethodReceiver::Instance));
    let body = &*cb.body.node;
    let exprs = match body {
        roundhouse::ExprNode::Seq { exprs } => exprs.clone(),
        // Single statement collapses to non-Seq; treat as one-element list.
        _ => vec![cb.body.clone()],
    };
    assert!(!exprs.is_empty(), "before_destroy should not be empty");
    // First (and only) statement: `comments.each { |c| c.destroy }`.
    let first = &exprs[0];
    let (method, block_present) = match &*first.node {
        roundhouse::ExprNode::Send { method, block, .. } => (method.as_str(), block.is_some()),
        other => panic!("expected each-Send in before_destroy; got {other:?}"),
    };
    assert_eq!(method, "each");
    assert!(block_present, "each call should carry a block");
}

// ---------------------------------------------------------------------------
// Typing-coverage probe — sibling of
// `inference_on_spinel_blog_runtime::untyped_subexpressions_baseline`,
// pointed at the post-lowering output of every lowerer applied to
// real-blog: models, views, and controllers.
//
// What's measured: count of Expr sub-expressions whose `ty` is None
// (or Ty::Var{...}) after lowering, summed across every method body
// in every lowered class. Single test, single invariant — the
// universal post-lowering IR is fully typed for emission. Failure
// path lists the first 20 sites with `Class#method` paths so the
// kind is implicit in the name.
// ---------------------------------------------------------------------------

fn collect_untyped_lowered(
    e: &roundhouse::expr::Expr,
    path: &str,
    out: &mut Vec<String>,
) {
    use roundhouse::expr::{ExprNode, InterpPart};
    use roundhouse::ty::Ty;

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
            collect_untyped_lowered(cond, &format!("{path}/if.cond"), out);
            collect_untyped_lowered(then_branch, &format!("{path}/if.then"), out);
            collect_untyped_lowered(else_branch, &format!("{path}/if.else"), out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_untyped_lowered(r, &format!("{path}/send.recv"), out);
            }
            for (i, a) in args.iter().enumerate() {
                collect_untyped_lowered(a, &format!("{path}/send.arg[{i}]"), out);
            }
            if let Some(b) = block {
                collect_untyped_lowered(b, &format!("{path}/send.block"), out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for (i, p) in parts.iter().enumerate() {
                if let InterpPart::Expr { expr } = p {
                    collect_untyped_lowered(expr, &format!("{path}/interp[{i}]"), out);
                }
            }
        }
        ExprNode::Seq { exprs } => {
            for (i, e) in exprs.iter().enumerate() {
                collect_untyped_lowered(e, &format!("{path}/seq[{i}]"), out);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_untyped_lowered(left, &format!("{path}/boolop.left"), out);
            collect_untyped_lowered(right, &format!("{path}/boolop.right"), out);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            collect_untyped_lowered(expr, &format!("{path}/rescue.expr"), out);
            collect_untyped_lowered(fallback, &format!("{path}/rescue.fallback"), out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_untyped_lowered(value, &format!("{path}/let.value"), out);
            collect_untyped_lowered(body, &format!("{path}/let.body"), out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_untyped_lowered(body, &format!("{path}/lambda.body"), out)
        }
        ExprNode::Apply { fun, args, block } => {
            collect_untyped_lowered(fun, &format!("{path}/apply.fun"), out);
            for (i, a) in args.iter().enumerate() {
                collect_untyped_lowered(a, &format!("{path}/apply.arg[{i}]"), out);
            }
            if let Some(b) = block {
                collect_untyped_lowered(b, &format!("{path}/apply.block"), out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (i, (k, v)) in entries.iter().enumerate() {
                collect_untyped_lowered(k, &format!("{path}/hash[{i}].key"), out);
                collect_untyped_lowered(v, &format!("{path}/hash[{i}].value"), out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for (i, el) in elements.iter().enumerate() {
                collect_untyped_lowered(el, &format!("{path}/array[{i}]"), out);
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_untyped_lowered(scrutinee, &format!("{path}/case.scrut"), out);
            for (i, arm) in arms.iter().enumerate() {
                if let Some(g) = &arm.guard {
                    collect_untyped_lowered(g, &format!("{path}/case.arm[{i}].guard"), out);
                }
                collect_untyped_lowered(&arm.body, &format!("{path}/case.arm[{i}].body"), out);
            }
        }
        ExprNode::Assign { value, .. } => {
            collect_untyped_lowered(value, &format!("{path}/assign.value"), out)
        }
        ExprNode::Yield { args } => {
            for (i, a) in args.iter().enumerate() {
                collect_untyped_lowered(a, &format!("{path}/yield.arg[{i}]"), out);
            }
        }
        ExprNode::Raise { value } => {
            collect_untyped_lowered(value, &format!("{path}/raise.value"), out)
        }
        ExprNode::Return { value } => {
            collect_untyped_lowered(value, &format!("{path}/return.value"), out)
        }
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for (i, a) in args.iter().enumerate() {
                    collect_untyped_lowered(a, &format!("{path}/super.arg[{i}]"), out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_untyped_lowered(body, &format!("{path}/begin.body"), out);
            for (i, r) in rescues.iter().enumerate() {
                for (j, c) in r.classes.iter().enumerate() {
                    collect_untyped_lowered(c, &format!("{path}/begin.rescue[{i}].class[{j}]"), out);
                }
                collect_untyped_lowered(&r.body, &format!("{path}/begin.rescue[{i}].body"), out);
            }
            if let Some(e) = else_branch {
                collect_untyped_lowered(e, &format!("{path}/begin.else"), out);
            }
            if let Some(e) = ensure {
                collect_untyped_lowered(e, &format!("{path}/begin.ensure"), out);
            }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value {
                collect_untyped_lowered(v, &format!("{path}/next.value"), out);
            }
        }
        ExprNode::MultiAssign { value, .. } => {
            collect_untyped_lowered(value, &format!("{path}/multi_assign.value"), out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_untyped_lowered(cond, &format!("{path}/while.cond"), out);
            collect_untyped_lowered(body, &format!("{path}/while.body"), out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_untyped_lowered(b, &format!("{path}/range.begin"), out);
            }
            if let Some(e) = end {
                collect_untyped_lowered(e, &format!("{path}/range.end"), out);
            }
        }
    }
}

/// Convert a slice of LibraryClasses into `(ClassId, ClassInfo)`
/// pairs suitable for passing as `extras` to a bulk lowerer. Folds
/// methods across same-named classes (e.g. `articles/index`,
/// `articles/show`, `articles/_article` all share `Views::Articles`)
/// before emitting. Each grouped entry is registered under both the
/// full ClassId and a last-segment alias so the body-typer's
/// Const-path resolver finds it.
fn build_class_info_extras(lcs: &[LibraryClass]) -> Vec<(ClassId, roundhouse::analyze::ClassInfo)> {
    use std::collections::HashMap;
    let mut grouped: HashMap<ClassId, roundhouse::analyze::ClassInfo> = HashMap::new();
    for lc in lcs {
        let info = grouped.entry(lc.name.clone()).or_default();
        let from = class_info_from_library_class(lc);
        for (k, v) in from.class_methods {
            info.class_methods.insert(k, v);
        }
        for (k, v) in from.instance_methods {
            info.instance_methods.insert(k, v);
        }
    }
    let mut out: Vec<(ClassId, roundhouse::analyze::ClassInfo)> = Vec::new();
    for (full_id, info) in grouped {
        let raw = full_id.0.as_str();
        let last = raw.rsplit("::").next().unwrap_or(raw).to_string();
        if last != raw {
            let mut alias = roundhouse::analyze::ClassInfo::default();
            alias.class_methods = info.class_methods.clone();
            alias.instance_methods = info.instance_methods.clone();
            out.push((ClassId(Symbol::from(last)), alias));
        }
        out.push((full_id, info));
    }
    out
}

#[test]
fn lowered_real_blog_typing_residual() {
    let app = ingest_app(fixture_path()).expect("ingest real-blog");

    // First pass: build view ClassInfo entries from per-view lowering
    // (cheap — only need the method signatures for the registry, not
    // typed bodies). Pass these as extras to the model lowerer.
    let preliminary_views: Vec<LibraryClass> = app
        .views
        .iter()
        .map(|v| lower_view_to_library_class(v, &app))
        .collect();
    let view_extras = build_class_info_extras(&preliminary_views);

    // Models go through the registry-returning bulk entry so
    // controllers and views can reuse the SAME registry — keeps the
    // ApplicationRecord baseline (find/all/where/etc) visible to
    // dispatch on Article.find(...).
    let (model_lcs, model_registry) =
        lower_models_with_registry(&app.models, &app.schema, view_extras);

    // Re-lower views via the bulk entry, passing the model registry
    // as extras. The bulk entry adds framework stubs (ViewHelpers,
    // RouteHelpers, Inflector, String) and runs body-typing with the
    // merged map so view bodies dispatch correctly on helpers and
    // sibling-view Sends.
    let view_lcs = lower_views_to_library_classes(
        &app.views,
        &app,
        model_registry.clone().into_iter().collect(),
    );

    // Controllers extend the model registry with views + their own
    // entries. Pass model_registry as extras so cross-class dispatch
    // (Article.find inside an action) sees the full baseline.
    let mut controller_extras: Vec<(ClassId, roundhouse::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    controller_extras.extend(build_class_info_extras(&view_lcs));
    let controller_lcs = lower_controllers_to_library_classes(&app.controllers, controller_extras);

    // Test modules — same shared-registry pattern.
    let mut test_extras: Vec<(ClassId, roundhouse::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    test_extras.extend(build_class_info_extras(&view_lcs));
    test_extras.extend(build_class_info_extras(&controller_lcs));
    let test_lcs = lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        test_extras,
    );

    let mut all_untyped: Vec<String> = Vec::new();
    let mut total_classes = 0usize;
    for lc in model_lcs
        .iter()
        .chain(&view_lcs)
        .chain(&controller_lcs)
        .chain(&test_lcs)
    {
        total_classes += 1;
        for method in &lc.methods {
            let path = format!("{}#{}", lc.name.0.as_str(), method.name.as_str());
            collect_untyped_lowered(&method.body, &path, &mut all_untyped);
        }
    }

    eprintln!(
        "lowered real-blog: {} untyped sub-expressions across {} classes \
         ({} models, {} views, {} controllers, {} test modules)",
        all_untyped.len(),
        total_classes,
        model_lcs.len(),
        view_lcs.len(),
        controller_lcs.len(),
        test_lcs.len(),
    );
    if std::env::var("DUMP_RESIDUAL").is_ok() {
        for (i, s) in all_untyped.iter().enumerate() {
            eprintln!("  {i}: {s}");
        }
    }

    // Floor reached on real-blog: 0 untyped sub-exprs across all 19
    // lowered classes (3 models, 9 views, 3 controllers, 4 test
    // modules). Tracker — fail loud on regression. Run `DUMP_RESIDUAL=1
    // cargo test ... -- --nocapture` to inspect.
    const CEILING: usize = 0;
    assert!(
        all_untyped.len() <= CEILING,
        "{} untyped sub-expressions on lowered real-blog — \
         exceeds ceiling of {CEILING}.\nFirst 20:\n  {}",
        all_untyped.len(),
        all_untyped
            .iter()
            .take(20)
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  "),
    );
}
