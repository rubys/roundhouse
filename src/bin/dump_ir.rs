//! Dump-IR — inspect lowered IR for a fixture, with selector and filter.
//!
//! Replaces the throwaway `eprintln!("{:#?}", method.body)` pattern with
//! a reusable binary that selects a subset of the lowered IR and dumps
//! it in one of three formats. Tracks the same lowering pipeline the
//! consolidated typing-residual test uses (model + view + controller
//! lowerers, shared registry, `lower_models_with_registry` →
//! `lower_views_to_library_classes` → `lower_controllers_to_library_classes`)
//! so what you see is what emit consumes.
//!
//! Usage:
//!
//!   cargo run --bin dump_ir -- <FIXTURE> [OPTIONS]
//!
//! Selector grammar (`--select`, default `*`):
//!
//!   Class             — whole class
//!   Class#method      — one method
//!   Class#*           — all methods on a class
//!   *#method          — same method across all classes
//!   *                 — everything
//!
//! Both halves accept glob-like `*` (and only `*`); for finer matches
//! pipe `--format json` through `jq`.
//!
//! Format (`--format`, default `debug`):
//!
//!   debug             — Rust `{:#?}` output. Verbose; the ground truth.
//!   ruby              — emit_method (per-method) or emit_library_class
//!                       (whole class). Readable; closer to "what does
//!                       this look like as code."
//!   json              — serde-serialized IR. Pipe through `jq`.
//!
//! Filters (`--filter`, may repeat — all must hold):
//!
//!   untyped           — only nodes whose subtree contains a sub-expr
//!                       with `ty: None` or `Ty::Var`. Same predicate
//!                       the typing-residual test uses.
//!   has-ivar:NAME     — only nodes whose subtree references `@NAME`.
//!   has-method:NAME   — only nodes whose subtree calls a Send named
//!                       NAME (any receiver).
//!
//! Examples:
//!
//!   cargo run --bin dump_ir -- fixtures/real-blog \
//!       --select ArticlesController#create
//!
//!   cargo run --bin dump_ir -- fixtures/real-blog \
//!       --select '*#create' --filter has-ivar:comment
//!
//!   cargo run --bin dump_ir -- fixtures/real-blog \
//!       --format json --select Article \
//!     | jq '.methods[] | select(.name == "validate")'

use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::dialect::{LibraryClass, MethodDef, MethodReceiver};
use roundhouse::expr::{Expr, ExprNode, InterpPart, LValue};
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app;
use roundhouse::lower::{
    class_info_from_library_class, lower_controllers_to_library_classes,
    lower_fixtures_to_library_classes, lower_models_with_registry,
    lower_test_modules_to_library_classes, lower_view_to_library_class,
    lower_views_to_library_classes,
};
use roundhouse::ty::Ty;

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!();
            print_usage();
            std::process::exit(2);
        }
    };

    let mut app = ingest_app(&opts.fixture).unwrap_or_else(|e| {
        eprintln!("ingest {}: {:?}", opts.fixture.display(), e);
        std::process::exit(1);
    });
    Analyzer::new(&app).analyze(&mut app);

    let lcs = lower_all(&app);

    let mut printed = 0usize;
    let mut sep = "";
    for lc in &lcs {
        if !opts.selector.matches_class(lc.name.0.as_str()) {
            continue;
        }
        let class_only = matches!(opts.selector, Selector::Class(_));
        if class_only {
            if !filters_pass_class(lc, &opts.filters) {
                continue;
            }
            print!("{sep}");
            print_class(lc, &opts);
            sep = "\n";
            printed += 1;
            continue;
        }
        for m in &lc.methods {
            if !opts.selector.matches_method(m.name.as_str()) {
                continue;
            }
            if !filters_pass_method(m, &opts.filters) {
                continue;
            }
            print!("{sep}");
            print_method(lc, m, &opts);
            sep = "\n";
            printed += 1;
        }
    }
    if printed == 0 {
        eprintln!("no IR matched selector + filters");
        std::process::exit(1);
    }
}

// ── CLI ──────────────────────────────────────────────────────────────

struct Opts {
    fixture: PathBuf,
    selector: Selector,
    format: Format,
    filters: Vec<Filter>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Debug,
    Ruby,
    Json,
}

enum Selector {
    All,
    Class(String),
    Method { class: String, method: String },
    AnyClassMethod(String),
    ClassAnyMethod(String),
}

impl Selector {
    fn matches_class(&self, name: &str) -> bool {
        match self {
            Self::All | Self::AnyClassMethod(_) => true,
            Self::Class(c) | Self::Method { class: c, .. } | Self::ClassAnyMethod(c) => {
                c == name
            }
        }
    }
    fn matches_method(&self, name: &str) -> bool {
        match self {
            Self::All | Self::ClassAnyMethod(_) => true,
            Self::AnyClassMethod(m) | Self::Method { method: m, .. } => m == name,
            Self::Class(_) => false, // class-only selector — caller handles
        }
    }
}

enum Filter {
    Untyped,
    HasIvar(String),
    HasMethod(String),
}

fn parse_args() -> Result<Opts, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        std::process::exit(0);
    }

    let mut selector_str = "*".to_string();
    let mut format = Format::Debug;
    let mut filters: Vec<Filter> = Vec::new();

    while let Some(i) = args.iter().position(|a| a.starts_with("--")) {
        let flag = args.remove(i);
        let val = if i < args.len() { args.remove(i) } else {
            return Err(format!("missing value for {flag}"));
        };
        match flag.as_str() {
            "--select" => selector_str = val,
            "--format" => {
                format = match val.as_str() {
                    "debug" => Format::Debug,
                    "ruby" => Format::Ruby,
                    "json" => Format::Json,
                    other => return Err(format!("unknown format: {other}")),
                };
            }
            "--filter" => filters.push(parse_filter(&val)?),
            other => return Err(format!("unknown flag: {other}")),
        }
    }

    let fixture = args
        .first()
        .cloned()
        .ok_or_else(|| "missing FIXTURE path".to_string())?;
    let fixture = PathBuf::from(fixture);

    let selector = parse_selector(&selector_str)?;

    Ok(Opts { fixture, selector, format, filters })
}

fn parse_selector(s: &str) -> Result<Selector, String> {
    if s == "*" {
        return Ok(Selector::All);
    }
    if let Some((class, method)) = s.split_once('#') {
        match (class, method) {
            ("*", "*") => Ok(Selector::All),
            ("*", m) => Ok(Selector::AnyClassMethod(m.to_string())),
            (c, "*") => Ok(Selector::ClassAnyMethod(c.to_string())),
            (c, m) => Ok(Selector::Method {
                class: c.to_string(),
                method: m.to_string(),
            }),
        }
    } else {
        Ok(Selector::Class(s.to_string()))
    }
}

fn parse_filter(s: &str) -> Result<Filter, String> {
    if s == "untyped" {
        return Ok(Filter::Untyped);
    }
    if let Some(name) = s.strip_prefix("has-ivar:") {
        return Ok(Filter::HasIvar(name.to_string()));
    }
    if let Some(name) = s.strip_prefix("has-method:") {
        return Ok(Filter::HasMethod(name.to_string()));
    }
    Err(format!("unknown filter: {s}"))
}

fn print_usage() {
    eprintln!("usage: dump_ir <FIXTURE> [--select PATTERN] [--format debug|ruby|json] [--filter F]...");
    eprintln!();
    eprintln!("  PATTERN: 'Class', 'Class#method', 'Class#*', '*#method', or '*'");
    eprintln!("  FILTER:  'untyped' | 'has-ivar:NAME' | 'has-method:NAME'");
}

// ── Pipeline (mirrors tests/model_lowerer.rs::lowered_real_blog_typing_residual) ─

fn lower_all(app: &roundhouse::App) -> Vec<LibraryClass> {
    let preliminary_views: Vec<LibraryClass> = app
        .views
        .iter()
        .map(|v| lower_view_to_library_class(v, app))
        .collect();
    let view_extras = build_class_info_extras(&preliminary_views);
    let (model_lcs, model_registry) =
        lower_models_with_registry(&app.models, &app.schema, view_extras);
    let view_lcs = lower_views_to_library_classes(
        &app.views,
        app,
        model_registry.clone().into_iter().collect(),
    );
    let mut controller_extras: Vec<(ClassId, roundhouse::analyze::ClassInfo)> =
        model_registry.clone().into_iter().collect();
    controller_extras.extend(build_class_info_extras(&view_lcs));
    let controller_lcs = lower_controllers_to_library_classes(&app.controllers, controller_extras);

    // Test modules — same shared-registry pattern. Test bodies dispatch
    // on models (`@article.title`), Comment.where(…), assertions on
    // self (Minitest::Test), so the registry needs all of: models +
    // views + controllers.
    let fixture_lcs = lower_fixtures_to_library_classes(app);

    let mut test_extras: Vec<(ClassId, roundhouse::analyze::ClassInfo)> =
        model_registry.into_iter().collect();
    test_extras.extend(build_class_info_extras(&view_lcs));
    test_extras.extend(build_class_info_extras(&controller_lcs));
    test_extras.extend(build_class_info_extras(&fixture_lcs));
    let test_lcs = lower_test_modules_to_library_classes(
        &app.test_modules,
        &app.fixtures,
        &app.models,
        test_extras,
    );

    let mut all = Vec::new();
    all.extend(model_lcs);
    all.extend(view_lcs);
    all.extend(controller_lcs);
    all.extend(fixture_lcs);
    all.extend(test_lcs);
    for lc in &app.library_classes {
        all.push(lc.clone());
    }
    all
}

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

// ── Filters ──────────────────────────────────────────────────────────

fn filters_pass_class(lc: &LibraryClass, filters: &[Filter]) -> bool {
    filters.iter().all(|f| match f {
        Filter::Untyped => lc.methods.iter().any(|m| has_untyped(&m.body)),
        Filter::HasIvar(name) => lc.methods.iter().any(|m| has_ivar(&m.body, name)),
        Filter::HasMethod(name) => lc.methods.iter().any(|m| has_send_method(&m.body, name)),
    })
}

fn filters_pass_method(m: &MethodDef, filters: &[Filter]) -> bool {
    filters.iter().all(|f| match f {
        Filter::Untyped => has_untyped(&m.body),
        Filter::HasIvar(name) => has_ivar(&m.body, name),
        Filter::HasMethod(name) => has_send_method(&m.body, name),
    })
}

fn has_untyped(e: &Expr) -> bool {
    let ty_ok = matches!(&e.ty, Some(t) if !matches!(t, Ty::Var { .. }));
    if !ty_ok {
        return true;
    }
    fold_subexprs(e, has_untyped)
}

fn has_ivar(e: &Expr, name: &str) -> bool {
    if let ExprNode::Ivar { name: n } = &*e.node {
        if n.as_str() == name {
            return true;
        }
    }
    if let ExprNode::Assign { target: LValue::Ivar { name: n }, .. } = &*e.node {
        if n.as_str() == name {
            return true;
        }
    }
    fold_subexprs(e, |c| has_ivar(c, name))
}

fn has_send_method(e: &Expr, name: &str) -> bool {
    if let ExprNode::Send { method, .. } = &*e.node {
        if method.as_str() == name {
            return true;
        }
    }
    fold_subexprs(e, |c| has_send_method(c, name))
}

/// Recurse into every direct sub-Expr child, returning true if any
/// child satisfies `pred`. Mirrors the structure of tests/model_lowerer
/// `collect_untyped_lowered`.
fn fold_subexprs(e: &Expr, mut pred: impl FnMut(&Expr) -> bool) -> bool {
    let mut any = false;
    visit_subexprs(e, &mut |c| {
        if !any && pred(c) {
            any = true;
        }
    });
    any
}

fn visit_subexprs(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match &*e.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef => {}
        ExprNode::If { cond, then_branch, else_branch } => {
            f(cond); visit_subexprs(cond, f);
            f(then_branch); visit_subexprs(then_branch, f);
            f(else_branch); visit_subexprs(else_branch, f);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv { f(r); visit_subexprs(r, f); }
            for a in args { f(a); visit_subexprs(a, f); }
            if let Some(b) = block { f(b); visit_subexprs(b, f); }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    f(expr); visit_subexprs(expr, f);
                }
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs { f(e); visit_subexprs(e, f); }
        }
        ExprNode::BoolOp { left, right, .. } => {
            f(left); visit_subexprs(left, f);
            f(right); visit_subexprs(right, f);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            f(expr); visit_subexprs(expr, f);
            f(fallback); visit_subexprs(fallback, f);
        }
        ExprNode::Let { value, body, .. } => {
            f(value); visit_subexprs(value, f);
            f(body); visit_subexprs(body, f);
        }
        ExprNode::Lambda { body, .. } => { f(body); visit_subexprs(body, f); }
        ExprNode::Apply { fun, args, block } => {
            f(fun); visit_subexprs(fun, f);
            for a in args { f(a); visit_subexprs(a, f); }
            if let Some(b) = block { f(b); visit_subexprs(b, f); }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                f(k); visit_subexprs(k, f);
                f(v); visit_subexprs(v, f);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements { f(el); visit_subexprs(el, f); }
        }
        ExprNode::Case { scrutinee, arms } => {
            f(scrutinee); visit_subexprs(scrutinee, f);
            for arm in arms {
                if let Some(g) = &arm.guard { f(g); visit_subexprs(g, f); }
                f(&arm.body); visit_subexprs(&arm.body, f);
            }
        }
        ExprNode::Assign { value, .. } => { f(value); visit_subexprs(value, f); }
        ExprNode::Yield { args } => {
            for a in args { f(a); visit_subexprs(a, f); }
        }
        ExprNode::Raise { value } => { f(value); visit_subexprs(value, f); }
        ExprNode::Return { value } => { f(value); visit_subexprs(value, f); }
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args { f(a); visit_subexprs(a, f); }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            f(body); visit_subexprs(body, f);
            for r in rescues {
                for c in &r.classes { f(c); visit_subexprs(c, f); }
                f(&r.body); visit_subexprs(&r.body, f);
            }
            if let Some(e) = else_branch { f(e); visit_subexprs(e, f); }
            if let Some(e) = ensure { f(e); visit_subexprs(e, f); }
        }
        ExprNode::Next { value } => {
            if let Some(v) = value { f(v); visit_subexprs(v, f); }
        }
        ExprNode::MultiAssign { value, .. } => { f(value); visit_subexprs(value, f); }
        ExprNode::While { cond, body, .. } => {
            f(cond); visit_subexprs(cond, f);
            f(body); visit_subexprs(body, f);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { f(b); visit_subexprs(b, f); }
            if let Some(e) = end { f(e); visit_subexprs(e, f); }
        }
    }
}

// ── Output ───────────────────────────────────────────────────────────

fn print_class(lc: &LibraryClass, opts: &Opts) {
    println!("// === {} ===", lc.name.0.as_str());
    match opts.format {
        Format::Debug => println!("{lc:#?}"),
        Format::Json => match serde_json::to_string_pretty(lc) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("json error: {e}"),
        },
        Format::Ruby => println!("{}", ruby_class(lc)),
    }
}

fn print_method(lc: &LibraryClass, m: &MethodDef, opts: &Opts) {
    let receiver = match m.receiver {
        MethodReceiver::Class => "static ",
        MethodReceiver::Instance => "",
    };
    println!(
        "// === {}#{}{}  (in {}) ===",
        lc.name.0.as_str(),
        receiver,
        m.name.as_str(),
        match m.receiver {
            MethodReceiver::Class => "class methods",
            MethodReceiver::Instance => "instance methods",
        },
    );
    match opts.format {
        Format::Debug => println!("{m:#?}"),
        Format::Json => match serde_json::to_string_pretty(m) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("json error: {e}"),
        },
        Format::Ruby => println!("{}", roundhouse::emit::ruby::emit_method(m)),
    }
}

/// Render a whole LibraryClass as Ruby by constructing a temporary
/// `App` containing only this class and routing through the Ruby
/// emitter's library path.
fn ruby_class(lc: &LibraryClass) -> String {
    let mut app = roundhouse::App::new();
    app.library_classes.push(lc.clone());
    let files = roundhouse::emit::ruby::emit_library(&app);
    files
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}
