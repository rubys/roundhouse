//! Lower a `TestModule` (one Ruby test file's `class XTest < Y` shape)
//! into a `LibraryClass` whose `methods` are one `def test_<snake>; …;
//! end` per `test "description" do … end` block. Output flows through
//! the universal walker like every other lowered class.
//!
//! The `test "name" do … end` macro form is just sugar for `def
//! test_<sanitized name>`, so the lowering is mostly mechanical:
//! sanitize the name, wrap the block body as a method body, tag the
//! kind as `Method`.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::dialect::{
    AccessorKind, Fixture, LibraryClass, MethodDef, MethodReceiver, Model, Test, TestModule,
};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::naming::{camelize, singularize};
use crate::span::Span;
use crate::ty::Ty;

/// Bulk entry. Lower every test module against a shared class
/// registry (typically the merged map from model + view + controller
/// lowerings) so test bodies dispatch on real receivers — `@article
/// .title` resolves to Article's title accessor, `Comment.where(…)`
/// resolves to the model's class methods.
pub fn lower_test_modules_to_library_classes(
    test_modules: &[TestModule],
    fixtures: &[Fixture],
    models: &[Model],
    extras: Vec<(ClassId, ClassInfo)>,
) -> Vec<LibraryClass> {
    let mut classes: HashMap<ClassId, ClassInfo> = HashMap::new();
    for (id, info) in extras {
        classes.insert(id, info);
    }
    // Same framework stubs the view + controller lowerers register —
    // RouteHelpers, ViewHelpers, Inflector, etc. Test bodies dispatch
    // on these via bare-name (`articles_url`) AND via Const
    // (`RouteHelpers.articles_url`). The mixin loop below copies the
    // RouteHelpers entries into each test class's instance_methods so
    // bare-name dispatch resolves.
    crate::lower::view_to_library::insert_framework_stubs(&mut classes);
    insert_minitest_test_baseline(&mut classes);

    // Fixture helpers — Rails mixes in `<table_name>(name: Sym) ->
    // Class(<Model>)` on every test class. Self-describing: derive
    // from app.fixtures + app.models so the registry knows what
    // `articles(:one)` returns.
    let fixture_helpers = build_fixture_helpers(fixtures, models);

    let mut out: Vec<LibraryClass> = Vec::new();
    let mut all_lcs: Vec<LibraryClass> = test_modules
        .iter()
        .map(build_library_class)
        .collect();

    // Self-info for each test class — its own test_* methods +
    // setup. Lets dispatch on `self` resolve when one test method
    // calls a setup helper (or when frameworks evolve to support
    // shared utility methods).
    for lc in &all_lcs {
        let mut info = ClassInfo::default();
        for m in &lc.methods {
            if let Some(sig) = &m.signature {
                info.instance_methods.insert(m.name.clone(), sig.clone());
                info.instance_method_kinds.insert(m.name.clone(), m.kind);
            }
        }
        // Inherit Minitest::Test assertion methods so `self.assert(...)`
        // dispatch resolves through the registry.
        for (name, sig) in MINITEST_INSTANCE_METHODS.iter() {
            let sym = Symbol::from(*name);
            info.instance_methods.entry(sym.clone()).or_insert_with(|| sig());
            info.instance_method_kinds
                .entry(sym)
                .or_insert(AccessorKind::Method);
        }
        // Mix in fixture helpers — `articles(:one)` returns Article.
        for (helper_name, sig) in &fixture_helpers {
            info.instance_methods.entry(helper_name.clone()).or_insert_with(|| sig.clone());
            info.instance_method_kinds
                .entry(helper_name.clone())
                .or_insert(AccessorKind::Method);
        }
        // Mix in route helpers — Rails makes `articles_url`,
        // `article_path`, etc. available on every test class via
        // include AbstractController::Routing::UrlFor. Pull from
        // the RouteHelpers stub registered by the view lowerer.
        if let Some(rh) = classes.get(&ClassId(Symbol::from("RouteHelpers"))) {
            for (helper_name, sig) in &rh.class_methods {
                info.instance_methods
                    .entry(helper_name.clone())
                    .or_insert_with(|| sig.clone());
                info.instance_method_kinds
                    .entry(helper_name.clone())
                    .or_insert(AccessorKind::Method);
            }
        }
        classes.insert(lc.name.clone(), info);
    }

    let empty_ivars: HashMap<Symbol, Ty> = HashMap::new();
    for lc in &mut all_lcs {
        for method in &mut lc.methods {
            crate::lower::typing::type_method_body(method, &classes, &empty_ivars);
        }
        out.push(lc.clone());
    }
    out
}

/// Single-module entry point — kept for tests/probes. For whole-app
/// emit, prefer the bulk entry which threads a shared registry.
pub fn lower_test_module_to_library_class(tm: &TestModule) -> LibraryClass {
    build_library_class(tm)
}

fn build_library_class(tm: &TestModule) -> LibraryClass {
    // Inline setup body at the start of every test method. The
    // body-typer's Seq walk picks up `@article = articles(:one)` and
    // propagates the type to downstream reads. Self-describing IR —
    // the assignment is materialized at every call site, just like
    // controller before-action filter inlining (ticket 8).
    let methods: Vec<MethodDef> = tm
        .tests
        .iter()
        .map(|t| test_to_method_def(&tm.name, t, tm.setup.as_ref()))
        .collect();
    LibraryClass {
        name: tm.name.clone(),
        is_module: false,
        parent: tm.parent.clone(),
        includes: Vec::new(),
        methods,
    }
}

/// Convert one `test "<name>" do …; end` block into `def
/// test_<snake_name>; <setup_body>; …; end`. The optional setup
/// argument — when present — gets prepended to the test body so
/// every test method is self-contained (no out-of-band setup
/// dependency, no double-call risk vs runtime auto-discovery).
fn test_to_method_def(owner: &ClassId, t: &Test, setup: Option<&Expr>) -> MethodDef {
    let snake = sanitize_test_name(&t.name);
    let method_name = Symbol::from(format!("test_{snake}"));
    let body = match setup {
        None => t.body.clone(),
        Some(s) => prepend_setup(s, &t.body),
    };
    MethodDef {
        name: method_name,
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: Some(crate::lower::typing::fn_sig(vec![], Ty::Nil)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    }
}

/// Concatenate setup statements + test body statements into one Seq.
/// Both sides are flattened (Seq-of-stmts → just the stmts) so the
/// resulting body has no nested Seqs that would block ivar
/// propagation through the typer's Seq walker (same reason
/// controller_to_library has flatten_seqs).
fn prepend_setup(setup: &Expr, body: &Expr) -> Expr {
    let mut stmts: Vec<Expr> = match &*setup.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![setup.clone()],
    };
    match &*body.node {
        ExprNode::Seq { exprs } => stmts.extend(exprs.iter().cloned()),
        _ => stmts.push(body.clone()),
    }
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts })
}

/// Map fixture file names to (helper_name, signature) pairs.
/// Rails's fixture helper convention: `<table_name>(name: Sym) ->
/// Class(<SingularModel>)`. Falls back to Untyped when the
/// corresponding model isn't registered (the helper still types as
/// callable, just less precisely).
fn build_fixture_helpers(fixtures: &[Fixture], models: &[Model]) -> Vec<(Symbol, Ty)> {
    let mut out: Vec<(Symbol, Ty)> = Vec::new();
    for f in fixtures {
        let plural_snake = f.name.as_str();
        let singular_snake = singularize(plural_snake);
        let class_name = camelize(&singular_snake);
        let resolved_class = models
            .iter()
            .find(|m| m.name.0.as_str() == class_name)
            .map(|_| Ty::Class { id: ClassId(Symbol::from(class_name)), args: vec![] })
            .unwrap_or(Ty::Untyped);
        let sig = crate::lower::typing::fn_sig(
            vec![(Symbol::from("name"), Ty::Sym)],
            resolved_class,
        );
        out.push((Symbol::from(plural_snake), sig));
    }
    out
}

/// `"creates an article with valid attributes"` →
/// `"creates_an_article_with_valid_attributes"`.
fn sanitize_test_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            for lower in c.to_lowercase() {
                out.push(lower);
            }
            prev_underscore = false;
        } else if !prev_underscore && !out.is_empty() {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Minitest::Test + ActiveSupport::TestCase + ActionDispatch::Integration-
/// Test instance methods every test body may call — assertions,
/// refutations, fixture/HTTP/response helpers. Loose signatures
/// (Untyped args) but concrete returns; lets the typer resolve
/// `self.assert(...)`, `self.get(url)`, `self.assert_response(...)`
/// dispatch through the registry. Combined here because Rails
/// fixtures don't separate them at the dispatch level — every test
/// class can reach all of these via the inheritance chain.
type SigBuilder = fn() -> Ty;
const MINITEST_INSTANCE_METHODS: &[(&str, SigBuilder)] = &[
    // Core Minitest assertions.
    ("assert", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_equal", || fn_sig_two(Ty::Untyped, Ty::Untyped, Ty::Nil)),
    ("assert_not", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_not_equal", || fn_sig_two(Ty::Untyped, Ty::Untyped, Ty::Nil)),
    ("assert_nil", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_not_nil", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_includes", || fn_sig_two(Ty::Untyped, Ty::Untyped, Ty::Nil)),
    ("assert_match", || fn_sig_two(Ty::Untyped, Ty::Untyped, Ty::Nil)),
    ("assert_raises", || fn_sig_one(Ty::Untyped, Ty::Untyped)),
    ("assert_difference", || fn_sig_one(Ty::Untyped, Ty::Untyped)),
    ("assert_no_difference", || fn_sig_one(Ty::Untyped, Ty::Untyped)),
    ("refute", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("refute_equal", || fn_sig_two(Ty::Untyped, Ty::Untyped, Ty::Nil)),
    ("refute_nil", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("skip", || fn_sig_one(Ty::Str, Ty::Nil)),
    ("flunk", || fn_sig_one(Ty::Str, Ty::Nil)),
    // ActionDispatch::IntegrationTest HTTP verbs — each takes a URL
    // (and possibly opts) and dispatches through the test rack stack.
    // Return Nil; sets `response`/`@response` ivars for downstream
    // assertions.
    ("get", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("post", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("put", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("patch", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("delete", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("head", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    // Response assertions.
    ("assert_response", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_redirected_to", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_select", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    ("assert_template", || fn_sig_one(Ty::Untyped, Ty::Nil)),
    // Response accessors.
    ("response", || crate::lower::typing::fn_sig(vec![], Ty::Untyped)),
    ("request", || crate::lower::typing::fn_sig(vec![], Ty::Untyped)),
    ("session", || crate::lower::typing::fn_sig(vec![], Ty::Untyped)),
    ("cookies", || crate::lower::typing::fn_sig(vec![], Ty::Untyped)),
    ("flash", || crate::lower::typing::fn_sig(vec![], Ty::Untyped)),
];

fn fn_sig_one(p: Ty, ret: Ty) -> Ty {
    crate::lower::typing::fn_sig(vec![(Symbol::from("arg"), p)], ret)
}

fn fn_sig_two(a: Ty, b: Ty, ret: Ty) -> Ty {
    crate::lower::typing::fn_sig(
        vec![(Symbol::from("a"), a), (Symbol::from("b"), b)],
        ret,
    )
}

/// Insert a `Minitest::Test` ClassInfo entry — the parent of every
/// test class. The test classes themselves register their inherited
/// methods into their own ClassInfo above; this stub is for callers
/// that look up `Minitest::Test` directly (e.g. via `Const { path:
/// [Minitest, Test] }`).
fn insert_minitest_test_baseline(classes: &mut HashMap<ClassId, ClassInfo>) {
    let mut info = ClassInfo::default();
    for (name, sig) in MINITEST_INSTANCE_METHODS.iter() {
        let sym = Symbol::from(*name);
        info.instance_methods.insert(sym.clone(), sig());
        info.instance_method_kinds.insert(sym, AccessorKind::Method);
    }
    classes.insert(
        ClassId(Symbol::from("Minitest::Test")),
        info.clone(),
    );
    // Last-segment alias for the typer's Const-path resolver.
    classes.insert(ClassId(Symbol::from("Test")), info.clone());
    // ActiveSupport::TestCase is the Rails-shape parent (extends
    // Minitest::Test under the hood); register the same surface.
    classes.insert(ClassId(Symbol::from("ActiveSupport::TestCase")), info.clone());
    classes.insert(ClassId(Symbol::from("TestCase")), info);
}
