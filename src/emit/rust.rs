//! Rust emit entry. Phase 7.3 (2026-05-20) collapsed the per-target
//! tree (`cargo`/`controller`/`fixture`/`importmap`/`main`/`model`/
//! `route`/`schema_sql`/`shared`/`spec`/`ty`/`view`) into a single
//! `pub fn emit` shim that delegates to `super::rust2::emit`. The
//! standalone `pub fn emit_method` runtime-extraction helper stays
//! — it's used by `tests/runtime_src_integration.rs` and has no
//! dependency on the deleted modules.
//!
//! Why keep `rust.rs` at all? Public callers
//! (`src/bin/build-site.rs`, `tests/preview_ts.rs`, the
//! `--target rust` flag in `src/bin/emit_preview.rs`,
//! `scripts/compare rust`) reach this through
//! `crate::emit::rust::emit`. Renaming would ripple across the call
//! sites + the user-facing CLI surface. Shim form keeps the
//! identity stable while the implementation lives in `rust2.rs`.

use std::fmt::Write;

use super::EmittedFile;
use super::rust2::ty::rust_ty;
use crate::App;
use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ty::Ty;

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
    // Analyzer-set diagnostic annotations short-circuit to a target
    // raise-equivalent (preserves Ruby's runtime-raise semantics).
    if e.diagnostic.is_some() {
        return r#"panic!("roundhouse: + with incompatible operand types")"#.to_string();
    }
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
        ExprNode::Cast { value, .. } => rt_emit_expr(value),
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

fn rt_emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    if let (Some(r), [arg]) = (recv, args) {
        // Comparison dispatch: Rust requires explicit cast on the
        // Int side for mixed Int/Float comparison (`1 as f64 < 2.0`).
        // SameType and Unknown fall through to native infix.
        if matches!(method, "<" | "<=" | ">" | ">=") {
            use crate::emit::shared::cmp::{classify_cmp, CmpCase};
            use crate::ty::Ty;
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_cmp(r, arg) {
                CmpCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("{ls} as f64"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("{rs} as f64")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} {method} {rs_cast}");
                }
                CmpCase::Incompatible => {
                    return format!(
                        r#"panic!("roundhouse: `{method}` with incompatible operand types")"#
                    );
                }
                CmpCase::ClassSubclass => {
                    return format!(
                        r#"panic!("roundhouse: `{method}` between Class refs not yet supported for Rust target")"#
                    );
                }
                CmpCase::SameType | CmpCase::Unknown => {}
            }
        }
        // `+` dispatch: Rust needs distinct emission for strings and
        // arrays because native `+` doesn't work on them.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            use crate::ty::Ty;
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_add(r, arg) {
                AddCase::StringConcat => {
                    return format!("format!(\"{{}}{{}}\", {ls}, {rs})");
                }
                AddCase::ArrayConcat { .. } => {
                    return format!("[&{ls}[..], &{rs}[..]].concat()");
                }
                AddCase::NumericPromote => {
                    // Cast the Int side to f64; Float is already f64.
                    let (ls_cast, rs_cast) =
                        match (r.ty.as_ref(), arg.ty.as_ref()) {
                            (Some(Ty::Int), _) => (format!("{ls} as f64"), rs),
                            (_, Some(Ty::Int)) => (ls, format!("{rs} as f64")),
                            _ => (ls, rs),
                        };
                    return format!("{ls_cast} + {rs_cast}");
                }
                AddCase::Incompatible => {
                    // Emit a runtime panic; Rust's `panic!()` has
                    // type `!`, so it's valid in any expression
                    // position.
                    return r#"panic!("roundhouse: + with incompatible operand types")"#.to_string();
                }
                AddCase::Numeric | AddCase::Unknown => {}
            }
        }
        // `-` dispatch: Rust's native `-` handles numerics (with a
        // cast on the Int side of a mixed pair). Array set-difference
        // has no native form — emit an iterator filter. Incompatible
        // pairs are refused.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { elem } => {
                    let elem_ty = rust_ty(elem);
                    return format!(
                        "{ls}.iter().filter(|x| !{rs}.contains(x)).cloned().collect::<Vec<{elem_ty}>>()"
                    );
                }
                SubCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("{ls} as f64"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("{rs} as f64")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} - {rs_cast}");
                }
                SubCase::Incompatible => {
                    return r#"panic!("roundhouse: - with incompatible operand types")"#.to_string();
                }
                SubCase::Numeric | SubCase::Unknown => {}
            }
        }
        // `*` dispatch: Rust's native `*` handles numerics (with a
        // cast on the Int side of a mixed pair). String/array repeat
        // use `.repeat()`. Array join uses `.join()` (elem Str) or a
        // to_string fan-out for other element types. Incompatible
        // pairs refuse.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_mul(r, arg) {
                MulCase::StringRepeat => {
                    return format!("{ls}.repeat({rs} as usize)");
                }
                MulCase::ArrayRepeat { .. } => {
                    return format!("{ls}.repeat({rs} as usize)");
                }
                MulCase::ArrayJoin { elem } => {
                    if matches!(elem, Ty::Str) {
                        return format!("{ls}.join(&{rs})");
                    } else {
                        return format!(
                            "{ls}.iter().map(|x| x.to_string()).collect::<Vec<String>>().join(&{rs})"
                        );
                    }
                }
                MulCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("{ls} as f64"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("{rs} as f64")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} * {rs_cast}");
                }
                MulCase::Incompatible => {
                    return r#"panic!("roundhouse: * with incompatible operand types")"#.to_string();
                }
                MulCase::Numeric | MulCase::Unknown => {}
            }
        }
        // `/` and `**` dispatch: both pure-numeric. `/` is native
        // infix; `**` is a method call (`.pow(n)` for Int, `.powf(n)`
        // for Float). Mixed numerics need explicit `as f64` casts.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_div_pow(r, arg) {
                DivPowCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("{ls} as f64"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("{rs} as f64")),
                        _ => (ls, rs),
                    };
                    if method == "**" {
                        return format!("{ls_cast}.powf({rs_cast})");
                    }
                    return format!("{ls_cast} / {rs_cast}");
                }
                DivPowCase::Numeric => {
                    if method == "**" {
                        // Pick integer-vs-float pow based on lhs type.
                        let is_float = matches!(r.ty.as_ref(), Some(Ty::Float));
                        let pow_m = if is_float { "powf" } else { "pow" };
                        // .pow takes u32 for integers; cast on the Int side.
                        let rs_cast = if is_float { rs } else { format!("{rs} as u32") };
                        return format!("{ls}.{pow_m}({rs_cast})");
                    }
                    // `/` falls through to native.
                }
                DivPowCase::Incompatible => {
                    return format!(
                        r#"panic!("roundhouse: `{method}` with incompatible operand types")"#
                    );
                }
                DivPowCase::Unknown => {}
            }
        }
        // `%` dispatch: numeric uses native `%` (with `as f64` cast
        // on mixed). Str % args is Ruby's sprintf — Rust has no direct
        // equivalent (format! needs compile-time format string), so
        // emit a runtime panic with a clear message.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            let ls = rt_emit_expr(r);
            let rs = rt_emit_expr(arg);
            match classify_modulo(r, arg) {
                ModuloCase::NumericPromote => {
                    let (ls_cast, rs_cast) = match (r.ty.as_ref(), arg.ty.as_ref()) {
                        (Some(Ty::Int), _) => (format!("{ls} as f64"), rs),
                        (_, Some(Ty::Int)) => (ls, format!("{rs} as f64")),
                        _ => (ls, rs),
                    };
                    return format!("{ls_cast} % {rs_cast}");
                }
                ModuloCase::StringFormat => {
                    return r#"panic!("roundhouse: String % (sprintf) not yet supported for Rust target")"#.to_string();
                }
                ModuloCase::Incompatible => {
                    return r#"panic!("roundhouse: % with incompatible operand types")"#.to_string();
                }
                ModuloCase::Numeric | ModuloCase::Unknown => {}
            }
        }
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
        Literal::Regex { pattern, flags } => {
            format!("regex::Regex::new({:?}).unwrap()", format!("(?{flags}){pattern}"))
        }
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

/// Phase 7.3 (2026-05-20): unconditional delegation to rust2. The
/// `ROUNDHOUSE_RUST_V2_LEGACY=1` escape hatch retires alongside the
/// legacy submodule files — there's nothing left to fall back to.
pub fn emit(app: &App) -> Vec<EmittedFile> {
    super::rust2::emit(app)
}
