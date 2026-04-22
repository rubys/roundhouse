//! Rust emitter.
//!
//! First-pass scope: emit each model as a plain struct with its attributes
//! as fields. No derives, no associations-as-references, no behavior — just
//! data shape. Extend incrementally; pressure from this emitter is what will
//! tell us where the IR and the analyzer need to grow.
//!
//! This is the first *typed* target. Unlike the Ruby emitter, output here
//! depends on `Ty` — `Str` → `String`, `Int` → `i64`, `Option<Ty::Nil, T>`
//! → `Option<T>`. The `ruby_emit_is_type_invariant` test deliberately
//! does NOT generalize to this emitter.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

mod cargo;
mod controller;
mod fixture;
mod importmap;
mod main;
mod model;
mod route;
mod schema_sql;
mod shared;
mod spec;
mod ty;
mod view;

pub use ty::rust_ty;

/// Source of the hand-written Roundhouse Rust runtime. Pulled in at
/// compile time from `runtime/rust/runtime.rs` so the file stays
/// editable as normal Rust (with its own tests, rust-analyzer support,
/// etc.) rather than living as a string constant here. When the
/// emitter runs, this string is copied verbatim into the generated
/// project's `src/runtime.rs`.
const RUNTIME_SOURCE: &str = include_str!("../../runtime/rust/runtime.rs");

/// Source of the Roundhouse Rust DB runtime. Same pattern as
/// `RUNTIME_SOURCE`: one hand-written file (`runtime/rust/db.rs`)
/// copied verbatim into the generated project as `src/db.rs`.
/// Owns the per-test SQLite connection.
const DB_SOURCE: &str = include_str!("../../runtime/rust/db.rs");

/// Source of the Roundhouse Rust HTTP runtime. Phase 4d: real axum-
/// backed helpers (`Params` with Rails-style bracketed-key strong
/// params, `redirect`, `html`, `unprocessable`, a `ViewCtx`
/// threaded through views). Copied verbatim into the generated
/// project as `src/http.rs` whenever any controller emits.
const HTTP_SOURCE: &str = include_str!("../../runtime/rust/http.rs");

/// Source of the test-support runtime. Provides the
/// `TestResponseExt` trait that emitted controller tests call into
/// (`assert_ok`, `assert_redirected_to`, `assert_select`, etc.).
/// Phase 4d ships substring-match implementations; a later upgrade
/// to a real CSS-selector engine only touches this file, emitted
/// tests stay the same.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/rust/test_support.rs");

/// Source of the view-helpers runtime. Supplies the Rails-compatible
/// helpers (`link_to`, `button_to`, `form_wrap`, FormBuilder
/// methods, etc.) that emitted view fns call into. Copied verbatim
/// into generated projects as `src/view_helpers.rs` alongside the
/// emitted `views.rs`.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/rust/view_helpers.rs");

/// Source of the server runtime. Axum startup, method-override
/// middleware, and layout wrap. Copied into the generated project
/// as `src/server.rs` so `main.rs` can `use app::server::start`.
const SERVER_SOURCE: &str = include_str!("../../runtime/rust/server.rs");

/// Source of the Action Cable runtime. Hand-written WebSocket
/// handler + Turbo Streams broadcaster. Shipped alongside the
/// server so `server::start` can mount `/cable` without a separate
/// compile-time feature flag.
const CABLE_SOURCE: &str = include_str!("../../runtime/rust/cable.rs");

/// Emit a typed `MethodDef` as a standalone `pub fn` Rust function
/// for the runtime-extraction pipeline. Self-contained walker for a
/// narrow Ruby subset (Lit / Var / Send with binary operators /
/// StringInterp / If). Runtime-authored code broader than this will
/// surface as a TODO in the emitted source and a test failure.
pub fn emit_method(m: &MethodDef) -> String {
    let sig = m
        .signature
        .as_ref()
        .expect("emit_method requires a signature");
    let Ty::Fn { params: sig_params, ret, .. } = sig else {
        panic!("signature is not Ty::Fn");
    };
    assert_eq!(
        sig_params.len(),
        m.params.len(),
        "method `{}`: signature/param arity mismatch",
        m.name
    );

    let param_list: Vec<String> = m
        .params
        .iter()
        .zip(sig_params.iter())
        .map(|(name, p)| format!("{}: {}", name, rust_param_ty(&p.ty)))
        .collect();

    let ret_s = rust_return_ty(ret);
    let body = rt_emit_expr(&m.body);

    let mut out = String::new();
    writeln!(
        out,
        "pub fn {}({}) -> {} {{",
        m.name,
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "    {line}").unwrap();
        }
    }
    out.push_str("}\n");
    out
}

/// Parameter-position type: strings borrowed (`&str`) by idiom.
fn rust_param_ty(ty: &Ty) -> String {
    match ty {
        Ty::Str => "&str".to_string(),
        _ => rust_ty(ty),
    }
}

/// Return-position type: strings owned (`String`), nil → `()`.
fn rust_return_ty(ty: &Ty) -> String {
    match ty {
        Ty::Nil => "()".to_string(),
        _ => rust_ty(ty),
    }
}

fn rt_emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => rt_emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::If { cond, then_branch, else_branch } => {
            // Rust's if-as-expression is native; braces required.
            let c = rt_emit_expr(cond);
            let t = rt_emit_expr(then_branch);
            let e = rt_emit_expr(else_branch);
            format!("if {c} {{\n    {t}\n}} else {{\n    {e}\n}}")
        }
        ExprNode::Send { recv, method, args, .. } => {
            rt_emit_send(recv.as_ref(), method.as_str(), args)
        }
        ExprNode::StringInterp { parts } => rt_emit_string_interp(parts),
        ExprNode::Seq { exprs } if exprs.len() == 1 => rt_emit_expr(&exprs[0]),
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

fn rt_emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    if let (Some(r), [arg]) = (recv, args) {
        if is_rust_binop(method) {
            return format!("{} {method} {}", rt_emit_expr(r), rt_emit_expr(arg));
        }
    }
    format!("/* TODO: send {method} */")
}

fn is_rust_binop(method: &str) -> bool {
    matches!(
        method,
        "==" | "!="
            | "<"
            | "<="
            | ">"
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "<<"
            | ">>"
            | "|"
            | "&"
            | "^"
    )
}

fn rt_emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "()".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
    }
}

fn rt_emit_string_interp(parts: &[InterpPart]) -> String {
    // Ruby `"x #{e} y"` → Rust `format!("x {} y", e)`. `{` and `}` in
    // literal text escape as `{{` / `}}`.
    let mut fmt = String::new();
    let mut args: Vec<String> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                for c in value.chars() {
                    if c == '{' || c == '}' {
                        fmt.push(c);
                        fmt.push(c);
                    } else {
                        fmt.push(c);
                    }
                }
            }
            InterpPart::Expr { expr } => {
                fmt.push_str("{}");
                args.push(rt_emit_expr(expr));
            }
        }
    }
    if args.is_empty() {
        format!("{fmt:?}.to_string()")
    } else {
        format!("format!({fmt:?}, {})", args.join(", "))
    }
}

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // Project skeleton: Cargo.toml + src/lib.rs. These tag along
    // unconditionally so the output is a self-contained Cargo project
    // the target toolchain will accept.
    files.push(cargo::emit_cargo_toml());

    if !app.models.is_empty() {
        files.push(model::emit_models(app));
        // The runtime tags along whenever any model is emitted —
        // every non-trivial app references at least
        // `crate::runtime::ValidationError` through the lowered
        // validation evaluator.
        files.push(EmittedFile {
            path: PathBuf::from("src/runtime.rs"),
            content: RUNTIME_SOURCE.to_string(),
        });
        // DB runtime — thread-local SQLite connection + helpers used
        // by save/destroy/count/find. Verbatim-copied, same posture
        // as `runtime.rs`.
        files.push(EmittedFile {
            path: PathBuf::from("src/db.rs"),
            content: DB_SOURCE.to_string(),
        });
        // Schema SQL — `CREATE TABLE` statements derived from the
        // ingested db/schema.rb. Phase 3 test harness uses this to
        // initialize a fresh :memory: SQLite database per test.
        files.push(schema_sql::emit_schema_sql(app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime — copied verbatim, same posture as `runtime.rs`
        // / `db.rs`. Provides `Params` / `redirect` / `html` helpers
        // used by emitted controllers and views.
        files.push(EmittedFile {
            path: PathBuf::from("src/http.rs"),
            content: HTTP_SOURCE.to_string(),
        });
        // Server runtime — axum startup, method-override middleware,
        // layout wrap. Referenced by the emitted `main.rs`.
        files.push(EmittedFile {
            path: PathBuf::from("src/server.rs"),
            content: SERVER_SOURCE.to_string(),
        });
        // Action Cable runtime — `/cable` WebSocket handler + Turbo
        // Streams broadcaster. Always shipped with controllers;
        // `server::start` mounts the route unconditionally so apps
        // using `<turbo-cable-stream-source>` subscribe cleanly.
        files.push(EmittedFile {
            path: PathBuf::from("src/cable.rs"),
            content: CABLE_SOURCE.to_string(),
        });
        files.push(main::emit_main_rs(app));
        files.push(importmap::emit_rust_importmap(app));
        // Test-support runtime — `TestResponseExt` trait consumed by
        // emitted controller tests. Only needed when tests emit, but
        // shipping it alongside controllers is simpler and harmless
        // (it only touches axum-test which is a dev-dep).
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.rs"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        let known_models: Vec<Symbol> =
            app.models.iter().map(|m| m.name.0.clone()).collect();
        for controller in &app.controllers {
            files.push(controller::emit_controller_axum(controller, app, &known_models));
        }
        files.push(controller::emit_controllers_mod(&app.controllers));
        // Router wiring the route table to the emitted action fns.
        files.push(route::emit_router(app));
        // Route helper functions (`articles_path()`, `article_path(
        // id)`, …) emitted from the route table.
        files.push(route::emit_route_helpers(app));
        // Views — real view fns derived from the ingested
        // `.html.erb` templates. `emit_views` walks the View IR's
        // `_buf = _buf + X` shape and renders per-statement into
        // Rust string-building. The view_helpers runtime provides
        // Rails-compatible helpers (link_to, form_with, render,
        // etc.).
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.rs"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(view::emit_views(app));
    }

    // Fixtures (test-only) — emit each YAML fixture as a Rust module
    // of labeled accessor functions returning struct instances. Used
    // by the generated tests below.
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        for f in &lowered.fixtures {
            files.push(fixture::emit_rust_fixture(f));
        }
        files.push(fixture::emit_fixtures_mod(&lowered));
    }

    // Tests — one Rust test module per Ruby test file.
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_rust_test_module(tm, app));
        }
        files.push(spec::emit_tests_mod(&app.test_modules));
    }

    // lib.rs declares the modules we emitted.
    files.push(cargo::emit_lib_rs(app));

    files
}
