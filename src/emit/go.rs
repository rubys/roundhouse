//! Go emitter.
//!
//! Second typed target. Differences from Rust that surface design choices:
//!
//! - Go has no Option / Result — nullable types are pointers (`*T`).
//! - Go has no class methods — `Post.All()` is pseudo-Go; a real mapping
//!   would use a repository pattern or package-level functions.
//! - Go requires explicit `return` for non-void functions and omits the
//!   return type entirely for void functions.
//! - Go convention: `ID`, `URL`, `HTTP` for initialism fields; PascalCase
//!   for exported identifiers, camelCase for unexported.
//! - Go uses tabs for indentation; gofmt would realign struct fields.
//!   We emit single-tab indent without alignment.
//!
//! Output is pseudo-Go — won't compile as-is. The goal is to prove that
//! types flow through to a second typed target without the Rust emitter
//! accidentally hiding ambiguities.
//!
//! Organized into one submodule per output kind. Cross-cutting helpers
//! live in `shared`; the generic body/expression walker lives in `expr`
//! and is reused by the model-method, view, and test emitters; type
//! rendering lives in `ty` (with `go_ty` re-exported here for any
//! external surface that may key off it).

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::ty::Ty;

mod controller;
mod controller_test;
mod expr;
mod fixture;
mod gomod;
mod importmap;
mod main;
mod model;
mod route;
mod schema_sql;
mod shared;
mod spec;
mod ty;
mod view;

// External API — `bin/build-site` consumes `emit`. `go_ty` is
// re-exported in case downstream callers key off it the way they
// do for `crystal_ty`.
pub use ty::go_ty;

const RUNTIME_SOURCE: &str = include_str!("../../runtime/go/runtime.go");
const DB_SOURCE: &str = include_str!("../../runtime/go/db.go");
/// Go HTTP runtime — Phase 4d pass-2 shape. Copied verbatim into
/// generated projects as `app/http.go` whenever any controller emits.
/// Provides ActionResponse, ActionContext, Router, plus Phase 4c
/// compile-only stubs for any leftover legacy references.
const HTTP_SOURCE: &str = include_str!("../../runtime/go/http.go");
/// Go test-support runtime — TestClient + TestResponse. Copied
/// verbatim as `app/test_support.go` whenever controllers emit.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/go/test_support.go");
/// Go view-helpers runtime — link_to, button_to, FormBuilder,
/// turbo_stream_from, dom_id, pluralize, plus set_yield/slot
/// storage for layout dispatch. Copied verbatim as
/// `app/view_helpers.go`.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/go/view_helpers.go");
/// Go net/http server runtime — `Start(StartOptions)` dispatches
/// through `Router.Match`, wraps HTML responses in the emitted
/// layout, handles `_method` override for Rails forms. Copied
/// verbatim as `app/server.go`.
const SERVER_SOURCE: &str = include_str!("../../runtime/go/server.go");
/// Go cable runtime — Action Cable WebSocket + Turbo Streams
/// broadcaster. Mirrors runtime/rust/cable.rs +
/// runtime/python/cable.py: actioncable-v1-json subprotocol,
/// per-channel subscriber map, partial-renderer registry. Copied
/// as `app/cable.go`.
const CABLE_SOURCE: &str = include_str!("../../runtime/go/cable.go");

/// Emit a typed `MethodDef` as a standalone Go function for the
/// runtime-extraction pipeline. Uses Go's idiomatic early-return form
/// for tail-position `If`; embedded `If` (anywhere other than tail or
/// RHS-of-assign) is rejected with a clear error instead of silently
/// producing an IIFE.
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
        .map(|(name, p)| format!("{} {}", name, go_ty(&p.ty)))
        .collect();

    let ret_s = go_ty(ret);
    let body = rt_emit_body(&m.body);

    let mut out = String::new();
    writeln!(
        out,
        "func {}({}) {} {{",
        go_export_name(m.name.as_str()),
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "\t{line}").unwrap();
        }
    }
    out.push_str("}\n");
    out
}

fn go_export_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Body emission runs in *return context*: whatever value the body
/// expression produces must be returned. Tail-position `If` is
/// rewritten to Go's early-return form (no `else` branch after the
/// `if` — Go's idiom). Nested tail `If`s cascade into else-if chains
/// via recursion.
fn rt_emit_body(body: &Expr) -> String {
    match &*body.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = rt_emit_expr(cond);
            let then_s = rt_emit_body(then_branch);
            let else_s = rt_emit_body(else_branch);
            let mut out = String::new();
            writeln!(out, "if {cond_s} {{").unwrap();
            for line in then_s.lines() {
                writeln!(out, "\t{line}").unwrap();
            }
            writeln!(out, "}}").unwrap();
            // Bare statements for the else side — Go's early-return idiom.
            out.push_str(&else_s);
            out
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            // Non-last stmts as statements; last stmt in return context.
            let mut out = String::new();
            let last = exprs.len() - 1;
            for (i, e) in exprs.iter().enumerate() {
                if i == last {
                    out.push_str(&rt_emit_body(e));
                } else {
                    // Other stmt shapes (Assign, etc.) arrive when runtime
                    // code actually uses them — for now, reject.
                    panic!("non-tail statement in method body not yet supported");
                }
            }
            out
        }
        _ => format!("return {}\n", rt_emit_expr(body)),
    }
}

fn rt_emit_expr(e: &Expr) -> String {
    // Analyzer-set diagnostic annotations short-circuit to a target
    // raise-equivalent (preserves Ruby's runtime-raise semantics).
    if e.diagnostic.is_some() {
        return r#"panic("roundhouse: + with incompatible operand types")"#.to_string();
    }
    match &*e.node {
        ExprNode::Lit { value } => rt_emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Send { recv, method, args, .. } => {
            rt_emit_send(recv.as_ref(), method.as_str(), args)
        }
        ExprNode::StringInterp { parts } => rt_emit_string_interp(parts),
        ExprNode::Seq { exprs } if exprs.len() == 1 => rt_emit_expr(&exprs[0]),
        ExprNode::If { .. } => {
            // Go has no ternary. A tail / assign-RHS If is lifted to
            // statement form by rt_emit_body; any other If is embedded
            // in mid-expression, which would need an IIFE — fail
            // loudly until a real case forces that work.
            panic!(
                "Go emit: ternary in embedded expression position — \
                 not yet supported (would need IIFE lowering)"
            )
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

fn rt_emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    if let (Some(r), [arg]) = (recv, args) {
        // Comparison dispatch: Go requires explicit cast on the
        // Int side for mixed Int/Float comparison (`float64(a) < b`).
        // SameType and Unknown fall through to native infix.
        if matches!(method, "<" | "<=" | ">" | ">=") {
            use crate::emit::shared::cmp::{classify_cmp, CmpCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_cmp(r, arg) {
                CmpCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("float64({ls})"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("float64({rs})")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} {method} {rs_cast}");
                }
                CmpCase::Incompatible => {
                    return format!(
                        r#"panic("roundhouse: `{method}` with incompatible operand types")"#
                    );
                }
                CmpCase::SameType | CmpCase::Unknown => {}
            }
        }
        // `+` dispatch: Go supports native `+` for numerics and
        // strings; Array concat needs `append(a, b...)`; mixed
        // numerics need an explicit cast.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_add(r, arg) {
                AddCase::ArrayConcat { .. } => {
                    return format!("append({ls}, {rs}...)");
                }
                AddCase::NumericPromote => {
                    // Cast the Int side to float64 (Float is already
                    // float64 in our go_ty mapping).
                    let (ls_cast, rs_cast) =
                        match (r.ty.as_ref(), arg.ty.as_ref()) {
                            (Some(Ty::Int), _) => (format!("float64({ls})"), rs),
                            (_, Some(Ty::Int)) => (ls, format!("float64({rs})")),
                            _ => (ls, rs),
                        };
                    return format!("{ls_cast} + {rs_cast}");
                }
                AddCase::Incompatible => {
                    // Emit a runtime panic. Go's `panic()` has no
                    // return type, so in expression position it
                    // produces a Go compile error — which is itself
                    // a loud, line-specific diagnostic. A typed
                    // helper (generics: `func rhIncompat[T](m) T`)
                    // would yield cleaner output; deferred until a
                    // runtime function actually triggers this case.
                    return r#"panic("roundhouse: + with incompatible operand types")"#.to_string();
                }
                AddCase::Numeric | AddCase::StringConcat | AddCase::Unknown => {}
            }
        }
        // `-` dispatch: Go supports native `-` for numerics; array
        // set-difference has no built-in, so emit an IIFE with a
        // nested loop. Mixed numerics need an explicit cast.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { elem } => {
                    let elem_ty = go_ty(elem);
                    return format!(
                        "func() []{elem_ty} {{ result := []{elem_ty}{{}}; for _, v := range {ls} {{ exclude := false; for _, w := range {rs} {{ if v == w {{ exclude = true; break }} }}; if !exclude {{ result = append(result, v) }} }}; return result }}()"
                    );
                }
                SubCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("float64({ls})"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("float64({rs})")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} - {rs_cast}");
                }
                SubCase::Incompatible => {
                    return r#"panic("roundhouse: - with incompatible operand types")"#.to_string();
                }
                SubCase::Numeric | SubCase::Unknown => {}
            }
        }
        if is_go_binop(method) {
            return format!(
                "{} {method} {}",
                rt_emit_expr(r),
                rt_emit_expr(arg)
            );
        }
    }
    format!("/* TODO: send {method} */")
}

fn is_go_binop(method: &str) -> bool {
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
        Literal::Nil => "nil".to_string(),
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
    // Ruby `"x #{e} y"` → Go `fmt.Sprintf("x <verb> y", e)` where the
    // verb depends on e's type (%s for string, %d for int, %g for
    // float, %v for everything else). Types come from the body-typer
    // populating `expr.ty` during parse_methods_with_rbs.
    let mut fmt = String::new();
    let mut args: Vec<String> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                for c in value.chars() {
                    if c == '%' {
                        fmt.push_str("%%");
                    } else {
                        fmt.push(c);
                    }
                }
            }
            InterpPart::Expr { expr } => {
                let verb = go_format_verb(expr);
                fmt.push_str(verb);
                args.push(rt_emit_expr(expr));
            }
        }
    }
    if args.is_empty() {
        format!("{fmt:?}")
    } else {
        format!("fmt.Sprintf({fmt:?}, {})", args.join(", "))
    }
}

fn go_format_verb(e: &Expr) -> &'static str {
    match e.ty.as_ref() {
        Some(Ty::Str | Ty::Sym) => "%s",
        Some(Ty::Int) => "%d",
        Some(Ty::Float) => "%g",
        Some(Ty::Bool) => "%t",
        _ => "%v",
    }
}

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(gomod::emit_go_mod());
    files.push(gomod::emit_go_sum());
    if !app.models.is_empty() {
        files.push(model::emit_models(app));
        files.push(EmittedFile {
            path: PathBuf::from("app/runtime.go"),
            content: RUNTIME_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/db.go"),
            content: DB_SOURCE.to_string(),
        });
        files.push(schema_sql::emit_schema_sql(app));
    }
    if !app.controllers.is_empty() {
        // HTTP runtime + TestClient — copied verbatim, same posture as
        // runtime.go / db.go. Provides ActionResponse, ActionContext,
        // Router, and (still) the Phase 4c stubs.
        files.push(EmittedFile {
            path: PathBuf::from("app/http.go"),
            content: HTTP_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/test_support.go"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/view_helpers.go"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/server.go"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("app/cable.go"),
            content: CABLE_SOURCE.to_string(),
        });
        let known_models: Vec<Symbol> =
            app.models.iter().map(|m| m.name.0.clone()).collect();
        for c in &app.controllers {
            files.push(controller::emit_controller_pass2(c, &known_models, app));
        }
        files.push(route::emit_go_route_helpers(app));
        files.push(route::emit_go_routes(app));
        files.push(importmap::emit_go_importmap(app));
        files.push(main::emit_go_main(app));
        files.push(view::emit_go_views(app, &known_models));
    }
    if !app.fixtures.is_empty() {
        files.push(fixture::emit_go_fixtures(app));
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_go_tests(tm, app));
        }
    }
    files
}
