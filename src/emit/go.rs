//! Go emit entry. Phase 6 step 3 (2026-05-24) collapsed the per-target
//! tree (`controller`/`gomod`/`importmap`/`main`/`model`/`route`/
//! `schema_sql`/`view`) into a thin `pub fn emit` shim that delegates
//! to `super::go2::emit_overlay_files`. Mirrors rust2's Phase 7.3
//! shape (src/emit/rust.rs, 2026-05-20).
//!
//! Why keep `go.rs` at all? Public callers — `src/project.rs::go_target`,
//! `tests/runtime_src_integration.rs::emit_method` — reach this through
//! `crate::emit::go::emit` / `::emit_method`. Renaming would ripple
//! across the call sites + the user-facing `--target go` CLI surface.
//! The shim keeps identity stable while the implementation lives in
//! `go2.rs`.
//!
//! Retained submodules:
//! - `shared` — go_field_name / go_method_name / pascalize_word, used
//!   by go2 emit
//! - `fixture` — emits `app/v2/fixtures_test.go` via go2's
//!   rewrite_test_file_to_v2
//! - `spec` — emits `app/v2/<model>_test.go` / `<controller>_test.go`
//! - `controller_test` — controller-test body classifier, used by spec
//! - `expr` — emit_literal helper, used by spec + controller_test
//! - `ty` — go_ty renderer, used by emit_method below

use std::fmt::Write;

use super::EmittedFile;
use crate::App;
use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ty::Ty;

mod expr;
pub(crate) mod fixture;
pub(crate) mod shared;
pub(crate) mod spec;
mod controller_test;
mod ty;

pub use ty::go_ty;

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
                CmpCase::ClassSubclass => {
                    return format!(
                        r#"panic("roundhouse: `{method}` between Class refs not yet supported for Go target")"#
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
        // `*` dispatch: Go supports native `*` for numerics; `strings
        // .Repeat` for Str*Int; array repeat and join need IIFEs
        // since Go has no built-in. Mixed numerics need explicit casts.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_mul(r, arg) {
                MulCase::StringRepeat => {
                    return format!("strings.Repeat({ls}, int({rs}))");
                }
                MulCase::ArrayRepeat { elem } => {
                    let elem_ty = go_ty(elem);
                    return format!(
                        "func() []{elem_ty} {{ r := []{elem_ty}{{}}; for i := 0; i < int({rs}); i++ {{ r = append(r, {ls}...) }}; return r }}()"
                    );
                }
                MulCase::ArrayJoin { elem } => {
                    if matches!(elem, Ty::Str) {
                        return format!("strings.Join({ls}, {rs})");
                    } else {
                        return format!(
                            "func() string {{ parts := make([]string, len({ls})); for i, v := range {ls} {{ parts[i] = fmt.Sprintf(\"%v\", v) }}; return strings.Join(parts, {rs}) }}()"
                        );
                    }
                }
                MulCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("float64({ls})"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("float64({rs})")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} * {rs_cast}");
                }
                MulCase::Incompatible => {
                    return r#"panic("roundhouse: * with incompatible operand types")"#.to_string();
                }
                MulCase::Numeric | MulCase::Unknown => {}
            }
        }
        // `/` and `**` dispatch: Go has native `/`; `**` needs
        // `math.Pow` (always float64-typed). Mixed numerics need casts.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_div_pow(r, arg) {
                DivPowCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("float64({ls})"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("float64({rs})")),
                        _ => (ls, rs),
                    };
                    if method == "**" {
                        return format!("math.Pow({ls_cast}, {rs_cast})");
                    }
                    return format!("{ls_cast} / {rs_cast}");
                }
                DivPowCase::Numeric => {
                    if method == "**" {
                        // Go's math.Pow takes float64 only.
                        return format!("math.Pow(float64({ls}), float64({rs}))");
                    }
                    // `/` falls through to native below.
                }
                DivPowCase::Incompatible => {
                    return format!(
                        r#"panic("roundhouse: `{method}` with incompatible operand types")"#
                    );
                }
                DivPowCase::Unknown => {}
            }
        }
        // `%` dispatch: Go's `%` works only on integers (not floats).
        // For Float%Float or mixed, need `math.Mod`. Str % args has
        // no direct Go equivalent — emit a runtime panic.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_modulo(r, arg) {
                ModuloCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("float64({ls})"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("float64({rs})")),
                        _ => (ls, rs),
                    };
                    return format!("math.Mod({ls_cast}, {rs_cast})");
                }
                ModuloCase::Numeric => {
                    // Float%Float needs math.Mod; Int%Int uses native.
                    if matches!(r.ty.as_ref(), Some(Ty::Float)) {
                        return format!("math.Mod({ls}, {rs})");
                    }
                    // Int%Int falls through to native `%`.
                }
                ModuloCase::StringFormat => {
                    return r#"panic("roundhouse: String % (sprintf) not yet supported for Go target")"#.to_string();
                }
                ModuloCase::Incompatible => {
                    return r#"panic("roundhouse: % with incompatible operand types")"#.to_string();
                }
                ModuloCase::Unknown => {}
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
        Literal::Regex { pattern, flags } => {
            format!("regexp.MustCompile({:?})", format!("(?{flags}){pattern}"))
        }
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

/// Emit the Go target. Delegates to `super::go2::emit_overlay_files`
/// (the strangler-fig path went unconditional in Phase 6 step 2;
/// Phase 6 step 3 deletes the legacy emit + this function shrinks to
/// a forward). `bin/rh transpile go` / `cargo run --bin emit_preview
/// -- --target go` both route through here. Output is now
/// `app/v2/*.go` + `cmd/v2/main.go` + a synthesized `main.go` +
/// `go.mod`/`go.sum`; the legacy single-package `app/*.go` shape is
/// retired.
pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = super::go2::emit_overlay_files(app);
    // go.mod + go.sum — small enough to inline here. Without these
    // `go build` / `go vet` won't resolve dependencies.
    files.push(emit_go_mod());
    files.push(emit_go_sum());
    // README + root main.go template shim. Pre-Phase-6 the root
    // main.go ran the legacy `app` package; with legacy retired it
    // forwards to `cmd/v2/`. Kept as a `package main` file so
    // `go build .` from the project root still produces a binary.
    files.push(emit_root_main());
    files.push(emit_readme());
    files
}

/// Standalone go.mod for the emitted project. Phase 6 step 2/3 left
/// the v2 overlay needing modernc.org/sqlite (db.go) — the legacy
/// nhooyr.io/websocket dep was the cable.go binding, retired here.
/// Future cable.go restoration would re-add the websocket dep when
/// it lands. Module name `app`; subpackages `app/v2`, `cmd/v2`.
fn emit_go_mod() -> EmittedFile {
    EmittedFile {
        path: std::path::PathBuf::from("go.mod"),
        content: "module app\n\ngo 1.24\n\nrequire modernc.org/sqlite v1.34.1\n".to_string(),
    }
}

/// Minimal go.sum placeholder — `go mod tidy` populates real hashes
/// on first build. Ship an empty file so the directory layout matches
/// a normal Go project (some tooling looks for go.sum's presence to
/// decide whether deps are vendored).
fn emit_go_sum() -> EmittedFile {
    EmittedFile {
        path: std::path::PathBuf::from("go.sum"),
        content: String::new(),
    }
}

/// Root `main.go` — forwards to `cmd/v2/main.go`. Kept so a default
/// `go build` from the project root produces a binary; existing
/// scripts (`scripts/compare go`, deployment templates) reference the
/// root path. Body matches cmd/v2/main.go's behavior.
fn emit_root_main() -> EmittedFile {
    EmittedFile {
        path: std::path::PathBuf::from("main.go"),
        content: "// Generated by Roundhouse.\n\
                  // Forward to the v2 server; cmd/v2/main.go contains the\n\
                  // same body. Kept at the root so `go build .` works for\n\
                  // legacy scripts that don't know to build from cmd/v2/.\n\
                  package main\n\n\
                  import (\n\
                  \t\"os\"\n\n\
                  \tv2 \"app/app/v2\"\n\
                  )\n\n\
                  func main() {\n\
                  \tv2.Server_start(v2.Router(), v2.StartOptions{\n\
                  \t\tDBPath:    os.Getenv(\"DATABASE_PATH\"),\n\
                  \t\tPort:      os.Getenv(\"PORT\"),\n\
                  \t\tSchemaSQL: v2.CreateTables,\n\
                  \t})\n\
                  }\n"
            .to_string(),
    }
}

fn emit_readme() -> EmittedFile {
    EmittedFile {
        path: std::path::PathBuf::from("README.md"),
        content: "# Generated Go App\n\n\
                  Built by Roundhouse from a Rails source app. Run:\n\n\
                  ```\n\
                  go mod tidy\n\
                  go build .\n\
                  ./app\n\
                  ```\n\n\
                  Or build the v2 binary explicitly:\n\n\
                  ```\n\
                  go build -o server ./cmd/v2/\n\
                  ./server\n\
                  ```\n"
            .to_string(),
    }
}
