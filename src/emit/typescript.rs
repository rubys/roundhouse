//! TypeScript emitter — rebuild in progress.
//!
//! Being rebuilt slice-by-slice against the spinel-blog canonical
//! output shape (see project_emitter_rip_and_replace memory). Each
//! commit lands one slice; the 32 ignored TS tests under tests/ are
//! the re-entry gate.
//!
//! Slice 1 (this revision): package.json + main.ts.

use std::fmt::Write;

use super::EmittedFile;
use crate::App;
use crate::ty::Ty;

mod expr;
mod main_ts;
mod model;
mod naming;
mod package;
mod route;
mod route_helpers;
mod ty;

pub use ty::ts_ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package::emit_package_json());
    files.push(main_ts::emit_main_ts(app));
    files.extend(model::emit_models(app));
    if !app.routes.entries.is_empty() {
        files.push(route::emit_routes(app));
        files.push(route_helpers::emit_route_helpers(app));
    }
    files
}

pub fn emit_library(_app: &App) -> Vec<EmittedFile> {
    Vec::new()
}

pub fn emit_with_adapter(
    app: &App,
    _adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    emit(app)
}

/// Emit a typed `MethodDef` as a standalone exported TypeScript
/// function (trailing newline included). Requires `signature` to be
/// populated — `parse_methods_with_rbs` does this. Used by the
/// runtime-extraction pipeline.
pub fn emit_method(m: &crate::dialect::MethodDef) -> String {
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
        .map(|(name, p)| format!("{}: {}", name, ts_ty(&p.ty)))
        .collect();

    let ret_s = ts_ty(ret);
    let body = expr::emit_body(&m.body, ret);

    let mut out = String::new();
    writeln!(
        out,
        "export function {}({}): {} {{",
        m.name,
        param_list.join(", "),
        ret_s
    )
    .unwrap();
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("}\n");
    out
}
