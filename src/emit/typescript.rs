//! TypeScript emitter — rebuild in progress.
//!
//! Being rebuilt slice-by-slice against the spinel-blog canonical
//! output shape (see project_emitter_rip_and_replace memory). Each
//! commit lands one slice; the 32 ignored TS tests under tests/ are
//! the re-entry gate.
//!
//! Slice 1 (this revision): package.json + main.ts.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::ty::Ty;

const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");
const HTTP_STUB_SOURCE: &str = include_str!("../../runtime/typescript/http.ts");
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/typescript/test_support.ts");
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/typescript/view_helpers.ts");
const SERVER_SOURCE: &str = include_str!("../../runtime/typescript/server.ts");

mod controller;
mod expr;
mod fixture;
mod library;
mod main_ts;
mod model;
mod model_from_library;
mod naming;
mod package;
mod route;
mod route_helpers;
mod schema_sql;
mod spec;
mod ty;
mod view;

pub use ty::ts_ty;

pub fn emit(app: &App) -> Vec<EmittedFile> {
    emit_with_adapter(app, &crate::adapter::SqliteAdapter)
}

pub fn emit_library(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package::emit_package_json());
    files.push(package::emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    files.extend(library::emit_library_classes(app));
    files.extend(library::emit_library_class_decls(app));
    files
}

pub fn emit_with_adapter(
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(package::emit_package_json());
    files.push(package::emit_tsconfig_json(app));
    files.push(main_ts::emit_main_ts(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    if !app.models.is_empty() {
        files.push(schema_sql::emit_schema_sql(app));
    }
    files.extend(model::emit_models(app));
    files.extend(library::emit_library_class_decls(app));
    if !app.controllers.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("src/http.ts"),
            content: HTTP_STUB_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.ts"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.ts"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/server.ts"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(controller::emit_ts_importmap(app));
        files.extend(controller::emit_controllers(app, adapter));
    }
    if !app.routes.entries.is_empty() {
        files.push(route::emit_routes(app));
        files.push(route_helpers::emit_route_helpers(app));
    }
    files.extend(view::emit_views(app));
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(fixture::emit_ts_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(fixture::emit_ts_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(spec::emit_ts_spec(tm, app));
        }
    }
    files
}

/// Emit a list of typed `MethodDef`s — produced by
/// `parse_methods_with_rbs` from a whole `.rb` + `.rbs` pair — as a
/// single TypeScript module file (trailing newline included).
///
/// Surface choice: when every method is `MethodReceiver::Class` (i.e.
/// the source was a module of `def self.*` helpers, like
/// `runtime/ruby/inflector.rb`), each method emits as a standalone
/// `export function`. The Ruby module name is absorbed into the import
/// path on the calling side. This matches the existing hand-written
/// shape (e.g. `export function pluralize` in
/// `runtime/typescript/view_helpers.ts`).
///
/// Other surface forms (mixin module → class with instance methods,
/// concrete class with state) are deferred to follow-up work; the
/// helper rejects them rather than emit half-correctly.
pub fn emit_module(methods: &[crate::dialect::MethodDef]) -> Result<String, String> {
    use crate::dialect::MethodReceiver;

    if methods.is_empty() {
        return Ok(String::new());
    }
    if !methods.iter().all(|m| matches!(m.receiver, MethodReceiver::Class)) {
        return Err(format!(
            "emit_module: only all-class-method modules supported so far; \
             saw mixed/instance methods (first instance: `{}`)",
            methods
                .iter()
                .find(|m| matches!(m.receiver, MethodReceiver::Instance))
                .map(|m| m.name.as_str())
                .unwrap_or("<none>"),
        ));
    }

    let mut out = String::new();
    for (i, m) in methods.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&emit_method(m));
    }
    Ok(out)
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
