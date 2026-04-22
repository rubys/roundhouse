//! TypeScript emitter — targets Juntos.
//!
//! Started as the Phase 2 scaffold; Phase 3 upgrades the output to
//! Juntos's runtime shape (the committed TS target per the strategy
//! memory). Changes land incrementally so each commit stays small:
//!
//! - **Models** (this commit): `extends ApplicationRecord`, `static
//!   table_name`, `static columns`. One file per model under
//!   `app/models/<snake>.ts`. Schema-derived instance fields drop
//!   from the class body — Juntos materializes them at runtime from
//!   the `columns` list, and declaring them statically would
//!   collide with the runtime accessors.
//! - Validations, associations, broadcasts: separate Phase 3 commits
//!   once this first shape is in place.
//! - Controllers + router + views: later Phase 3 commits.
//!
//! Ruby → Juntos translation rules come from ruby2js's
//! `lib/ruby2js/filter/rails/model.rb` and `lib/ruby2js/filter/rails/
//! active_record.rb`. Those are the reference; our job is to produce
//! equivalent output driven by the typed IR.
//!
//! Non-goals still (later Phase 3 commits):
//! - Controller shape (extends Controller, ivar-style state).
//! - Router emit (Router.resources calls, not a flat table).
//! - View / template emission.
//! - `tsc --strict` cleanliness.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::{
    RouteSpec, Test, TestModule,
};
use crate::ident::Symbol;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

mod controller;
mod model;
mod view;

/// Hand-written Juntos-shape stub, copied into every generated project
/// as `src/juntos.ts`. tsconfig's `paths` alias rewrites `"juntos"`
/// imports to this file for type-checking without requiring npm
/// install. Real deployments swap in the actual Juntos package.
const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");

/// TypeScript HTTP runtime — Phase 4c compile-only stubs. Copied
/// verbatim into generated projects as `src/http.ts` when any
/// controller emits. Mirrors the Rust/Crystal/Go/Elixir twins.
const HTTP_STUB_SOURCE: &str = include_str!("../../runtime/typescript/http.ts");

/// Pass-2 test-support runtime. `TestClient` + `TestResponse` with
/// assertion methods mirroring Rust's `TestResponseExt` trait.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/typescript/test_support.ts");

/// Pass-2 view-helpers runtime. Rails-compatible `linkTo`,
/// `buttonTo`, `formWrap`, `FormBuilder`, `turboStreamFrom`, etc.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/typescript/view_helpers.ts");

/// HTTP + Action Cable server runtime. Copied into generated
/// projects as `src/server.ts`. Consumed by `main.ts` to start
/// the HTTP listener + WebSocket upgrade handler.
const SERVER_SOURCE: &str = include_str!("../../runtime/typescript/server.ts");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    // Default adapter for backward-compatible callers. Matches
    // pre-adapter-consumption behavior — nothing suspends, no
    // awaits beyond the `async function` wrapper that was already
    // there.
    emit_with_adapter(app, &crate::adapter::SqliteAdapter)
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
    let body = emit_body(&m.body, ret);

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

/// Emit with an explicit adapter. Async-capable targets (this one,
/// eventually Rust and Python) consult the adapter's
/// `is_suspending_effect` per Send site and insert `await` where
/// effects suspend. `SqliteAdapter` suspends nothing; `SqliteAsync
/// Adapter` suspends on DB effects — emit a fully-awaited body
/// that can later be pointed at a real async backend (IndexedDB,
/// D1, pg-on-Node) without further emitter changes.
pub fn emit_with_adapter(
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(emit_package_json());
    files.push(emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    if !app.models.is_empty() {
        files.push(emit_schema_sql_ts(app));
    }
    files.extend(model::emit_models(app));
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
        files.push(controller::emit_ts_route_helpers(app));
        // Always emit `src/importmap.ts` — empty PINS list when
        // the app has no `config/importmap.rb` — so the layout's
        // import line never fails to resolve.
        files.push(controller::emit_ts_importmap(app));
        files.extend(controller::emit_controllers(app, adapter));
        // Note: db/seeds.ts emission deferred. The top-level Ruby
        // transpile path (needed for seeds.rb → runnable TS)
        // requires more careful handling than the controller-body
        // emitter provides today: operator methods (`==` → `===`),
        // bang-stripping on class methods (`Article.create!` →
        // `Article.create`), and statement-structure preservation
        // through nested `if`/`unless` guards. See App::seeds for
        // the ingested expression; Ruby emit round-trips
        // correctly. TS emission picks up in a later bite.
        files.push(controller::emit_main_ts(app));
    }
    files.extend(view::emit_views(app));
    if !app.routes.entries.is_empty() {
        files.push(emit_routes(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(emit_ts_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(emit_ts_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(emit_ts_spec(tm, app));
        }
    }
    files
}

/// Minimal package.json. `"type": "module"` matches the ESM import/
/// export style the emitter produces. `@types/node` is required so
/// tsc can resolve `node:test` / `node:assert/strict` imports in the
/// emitted spec files. The tsconfig `paths` alias resolves `"juntos"`
/// to our local stub.
fn emit_package_json() -> EmittedFile {
    let content = "\
{
  \"name\": \"app\",
  \"version\": \"0.1.0\",
  \"private\": true,
  \"type\": \"module\",
  \"scripts\": {
    \"start\": \"tsx main.ts\"
  },
  \"dependencies\": {
    \"better-sqlite3\": \"^11.5.0\",
    \"ws\": \"^8.18.0\"
  },
  \"devDependencies\": {
    \"@types/node\": \"^20\",
    \"@types/better-sqlite3\": \"^7.6.0\",
    \"@types/ws\": \"^8.5.0\",
    \"typescript\": \"5.7.3\",
    \"tsx\": \"4.19.2\"
  }
}
";
    EmittedFile {
        path: PathBuf::from("package.json"),
        content: content.to_string(),
    }
}

/// `src/schema_sql.ts` — TypeScript constant wrapping the target-neutral
/// DDL produced by `lower::lower_schema`. `setupTestDb` executes the
/// string via `better-sqlite3`'s `Database#exec` (which handles
/// multi-statement batches natively).
fn emit_schema_sql_ts(app: &App) -> EmittedFile {
    let ddl = crate::lower::lower_schema(&app.schema);
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "export const CREATE_TABLES = `").unwrap();
    s.push_str(&ddl);
    writeln!(s, "`;").unwrap();
    EmittedFile {
        path: PathBuf::from("src/schema_sql.ts"),
        content: s,
    }
}

/// tsconfig.json — strict TS with the two bits that matter for the
/// generated shape: `paths` maps `"juntos"` to the local stub, and
/// `allowJs`/`esModuleInterop` let imports in both styles resolve.
/// As of Phase 4c controllers + http runtime land in the include list
/// since they compile against the `Roundhouse.Http` stubs; views and
/// routes still wait for their own runtime.
fn emit_tsconfig_json(app: &App) -> EmittedFile {
    let mut includes = String::from("\"app/models/**/*.ts\", \"src/juntos.ts\"");
    if !app.models.is_empty() {
        includes.push_str(", \"src/schema_sql.ts\"");
    }
    if !app.controllers.is_empty() {
        includes.push_str(
            ", \"app/controllers/**/*.ts\", \"app/views/**/*.ts\", \"src/http.ts\", \"src/test_support.ts\", \"src/view_helpers.ts\", \"src/route_helpers.ts\", \"src/routes.ts\", \"src/server.ts\", \"main.ts\"",
        );
    }
    if !app.test_modules.is_empty() || !app.fixtures.is_empty() {
        includes.push_str(", \"spec/**/*.ts\"");
    }
    let content = format!(
        "{{
  \"compilerOptions\": {{
    \"target\": \"ES2022\",
    \"module\": \"ESNext\",
    \"moduleResolution\": \"bundler\",
    \"strict\": false,
    \"esModuleInterop\": true,
    \"skipLibCheck\": true,
    \"noEmit\": true,
    \"baseUrl\": \".\",
    \"paths\": {{
      \"juntos\": [\"./src/juntos.ts\"]
    }}
  }},
  \"include\": [{includes}]
}}
"
    );
    EmittedFile {
        path: PathBuf::from("tsconfig.json"),
        content,
    }
}

// Routes ---------------------------------------------------------------

/// Emit the Juntos-shaped routes file: `Router.root(...)` for any
/// `root "c#a"` entries and `Router.resources("name", XController,
/// { nested: [...] })` for each `resources :name` block. Per-
/// controller imports go at the top. Matches ruby2js's rails routes
/// filter (`spec/rails_routes_spec.rb`).
fn emit_routes(app: &App) -> EmittedFile {
    // Collect every controller class-name the routes reference, so
    // we can emit one import per controller. Uses a Vec (preserving
    // source order) with de-duplication; a HashSet would lose
    // ordering and the deterministic import layout is nicer for
    // source-equivalence down the road.
    let mut controllers: Vec<String> = Vec::new();
    let mut push_controller = |name: String, out: &mut Vec<String>| {
        if !out.iter().any(|c| c == &name) {
            out.push(name);
        }
    };
    for entry in &app.routes.entries {
        collect_controller_refs(entry, &mut controllers, &mut push_controller);
    }

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "import {{ Router }} from \"juntos\";").unwrap();
    for controller in &controllers {
        let file_stem = crate::naming::snake_case(controller);
        writeln!(
            s,
            "import {{ {controller} }} from \"../app/controllers/{file_stem}.js\";"
        )
        .unwrap();
    }
    writeln!(s).unwrap();
    for entry in &app.routes.entries {
        emit_route_spec(&mut s, entry);
    }
    EmittedFile { path: PathBuf::from("src/routes.ts"), content: s }
}

/// Walk the route spec tree and push every controller class-name
/// into `out` (via the dedup closure). Follows nested resources.
fn collect_controller_refs(
    spec: &RouteSpec,
    out: &mut Vec<String>,
    push: &mut impl FnMut(String, &mut Vec<String>),
) {
    match spec {
        RouteSpec::Explicit { controller, .. } => {
            push(controller.0.to_string(), out);
        }
        RouteSpec::Root { target } => {
            if let Some((c, _)) = target.split_once('#') {
                push(controller_class_name(c), out);
            }
        }
        RouteSpec::Resources { name, nested, .. } => {
            push(controller_class_name(name.as_str()), out);
            for child in nested {
                collect_controller_refs(child, out, push);
            }
        }
    }
}

/// Emit one route spec as a Router.* call. Nested resources appear
/// as a `{ nested: [...] }` option list on the outer resources call.
fn emit_route_spec(out: &mut String, spec: &RouteSpec) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, .. } => {
            // Ruby2js doesn't have a direct Router shortcut for raw
            // verb routes; emit a low-level `Router.add` call so the
            // route still registers. No fixture exercises this path
            // (tiny-blog has explicit routes; real-blog uses
            // resources) — sharpen when one does.
            let verb = match method {
                crate::dialect::HttpMethod::Get => "get",
                crate::dialect::HttpMethod::Post => "post",
                crate::dialect::HttpMethod::Put => "put",
                crate::dialect::HttpMethod::Patch => "patch",
                crate::dialect::HttpMethod::Delete => "delete",
                crate::dialect::HttpMethod::Head => "head",
                crate::dialect::HttpMethod::Options => "options",
                crate::dialect::HttpMethod::Any => "match",
            };
            writeln!(
                out,
                "Router.{verb}({:?}, {}, {:?});",
                path,
                controller.0,
                action.as_str(),
            )
            .unwrap();
        }
        RouteSpec::Root { target } => {
            let (controller, action) = target
                .split_once('#')
                .map(|(c, a)| (controller_class_name(c), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            writeln!(out, "Router.root(\"/\", {controller}, {action:?});").unwrap();
        }
        RouteSpec::Resources { name, only, except: _, nested } => {
            let controller = controller_class_name(name.as_str());
            let mut opts: Vec<String> = Vec::new();
            if !only.is_empty() {
                let parts: Vec<String> =
                    only.iter().map(|s| format!("{:?}", s.as_str())).collect();
                opts.push(format!("only: [{}]", parts.join(", ")));
            }
            // except: is handled at build time per ruby2js's filter —
            // not passed to Router.resources at runtime. We mirror.
            if !nested.is_empty() {
                let mut nested_parts: Vec<String> = Vec::new();
                for child in nested {
                    if let Some(part) = nested_spec_entry(child) {
                        nested_parts.push(part);
                    }
                }
                if !nested_parts.is_empty() {
                    opts.push(format!("nested: [{}]", nested_parts.join(", ")));
                }
            }
            if opts.is_empty() {
                writeln!(out, "Router.resources({:?}, {});", name.as_str(), controller).unwrap();
            } else {
                writeln!(
                    out,
                    "Router.resources({:?}, {}, {{{}}});",
                    name.as_str(),
                    controller,
                    opts.join(", "),
                )
                .unwrap();
            }
        }
    }
}

/// Turn a nested child (from `resources :x do ... end`) into the
/// object-literal entry Juntos expects inside the `nested: [...]`
/// array: `{ name: "comments", controller: CommentsController, only: [...] }`.
fn nested_spec_entry(spec: &RouteSpec) -> Option<String> {
    let RouteSpec::Resources { name, only, except: _, nested, .. } = spec else {
        // Non-resources entries inside a nested block (e.g. raw
        // `get "/x"`) don't have a direct Router.resources nested
        // shape; skip for now — no fixture uses this pattern.
        return None;
    };
    let controller = controller_class_name(name.as_str());
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("name: {:?}", name.as_str()));
    parts.push(format!("controller: {controller}"));
    if !only.is_empty() {
        let os: Vec<String> = only.iter().map(|s| format!("{:?}", s.as_str())).collect();
        parts.push(format!("only: [{}]", os.join(", ")));
    }
    if !nested.is_empty() {
        let mut inner: Vec<String> = Vec::new();
        for child in nested {
            if let Some(p) = nested_spec_entry(child) {
                inner.push(p);
            }
        }
        if !inner.is_empty() {
            parts.push(format!("nested: [{}]", inner.join(", ")));
        }
    }
    Some(format!("{{{}}}", parts.join(", ")))
}

fn controller_class_name(short: &str) -> String {
    let mut s = crate::naming::camelize(short);
    s.push_str("Controller");
    s
}

// Body + expressions ---------------------------------------------------

pub(super) fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, value } => {
            if is_void {
                format!("{};", emit_expr(value))
            } else {
                format!("return {};", emit_expr(value))
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                lines.push(emit_stmt(e, i == exprs.len() - 1, is_void));
            }
            lines.join("\n")
        }
        _ => {
            if is_void {
                format!("{};", emit_expr(body))
            } else {
                format!("return {};", emit_expr(body))
            }
        }
    }
}

fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("const {} = {};", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("this.{} = {};", ts_field_name(name.as_str()), emit_expr(value))
        }
        _ => {
            if is_last && !void_return {
                format!("return {};", emit_expr(e))
            } else {
                format!("{};", emit_expr(e))
            }
        }
    }
}

pub(super) fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("this.{}", ts_field_name(name.as_str())),
        ExprNode::Send { recv, method, args, parenthesized, .. } => {
            emit_send_with_parens(recv.as_ref(), method.as_str(), args, *parenthesized)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // TS ternary `cond ? a : b`. `emit_expr` is always called in
            // an expression position; controller/view emitters have
            // their own statement-form If handlers.
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("{cond_s} ? {then_s} : {else_s}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '`' || c == '\\' {
                                out.push('\\');
                                out.push(c);
                            } else if c == '$' {
                                out.push_str("\\$");
                            } else {
                                out.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

/// Core send emission. `parenthesized` reflects whether the Ruby
/// source wrapped args in explicit parens — for 0-arg explicit-
/// receiver calls we use it to decide between `recv.name` (Ruby
/// reader convention, JS property access) and `recv.name()` (method
/// call). Always emits parens when args are present.
pub(super) fn emit_send_with_parens(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // Ruby's binary operators ride the Send channel. TS needs infix;
    // `==` and `!=` map to strict `===` / `!==` so equality semantics
    // match Ruby (Ruby has no implicit type coercion).
    if let (Some(r), [arg]) = (recv, args) {
        if let Some(op) = ts_binop(method) {
            return format!("{} {op} {}", emit_expr(r), emit_expr(arg));
        }
    }
    // Ruby stdlib method → TS equivalent, when the Ruby name collides
    // with a nonexistent TS property. Keyed on name only today; a
    // receiver-typed dispatch would replace this when per-type
    // mappings diverge.
    let (mapped_name, force_parens) = match method {
        "strip" => ("trim", true),
        _ => (method, false),
    };
    let ts_m = ts_method_name(mapped_name);
    match recv {
        None => {
            if args_s.is_empty() {
                ts_m
            } else {
                format!("{}({})", ts_m, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() && !parenthesized && !force_parens {
                // Ruby's `obj.name` without parens is typically a
                // reader; Juntos mirrors that with a property
                // accessor / getter, so emit without parens.
                format!("{recv_s}.{ts_m}")
            } else {
                format!("{recv_s}.{ts_m}({})", args_s.join(", "))
            }
        }
    }
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Ruby symbols map to string literals — the typed analyzer may
        // refine this into a discriminated-union enum later, but for
        // the scaffold a string is unambiguous and round-trips through
        // comparison as expected.
        Literal::Sym { value } => format!("{:?}", value.as_str()),
    }
}

// Types ----------------------------------------------------------------

pub fn ts_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Float => "number".to_string(),
        Ty::Bool => "boolean".to_string(),
        // Symbols model as string for now. When a pass identifies a
        // closed set of symbols at a given position (enum detection),
        // emit it as a union-of-string-literals instead.
        Ty::Str | Ty::Sym => "string".to_string(),
        Ty::Nil => "null".to_string(),
        Ty::Array { elem } => format!("{}[]", ts_ty(elem)),
        Ty::Hash { key, value } => {
            format!("Record<{}, {}>", ts_ty(key), ts_ty(value))
        }
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(ts_ty).collect();
            format!("[{}]", parts.join(", "))
        }
        Ty::Record { .. } => "Record<string, unknown>".to_string(),
        Ty::Union { variants } => {
            let parts: Vec<String> = variants.iter().map(ts_ty).collect();
            parts.join(" | ")
        }
        Ty::Class { id, .. } => id.0.to_string(),
        Ty::Fn { .. } => "(...args: unknown[]) => unknown".to_string(),
        Ty::Var { .. } => "unknown".to_string(),
    }
}

// Naming ---------------------------------------------------------------

/// Instance-field name: preserves snake_case. Juntos's ActiveRecord
/// accessors match the Rails column names exactly (`article_id`, not
/// `articleId`), and ruby2js's rails model filter does the same.
/// Single-word idiomatic JS (`title`) is the same either way; the
/// difference is only visible on multi-word names.
fn ts_field_name(ruby_name: &str) -> String {
    ruby_name.to_string()
}

/// Method name: same snake_case preservation as fields. Method calls
/// that should resolve to JS-native APIs (e.g. `findBy` vs Ruby's
/// `find_by`) will need a per-method translation table later; until
/// then, the Rails-side name survives and Juntos maps at runtime.
pub(super) fn ts_method_name(ruby_name: &str) -> String {
    ruby_name.to_string()
}

fn ts_binop(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "===",
        "!=" => "!==",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        "**" => "**",
        "<<" => "<<",
        ">>" => ">>",
        "|" => "|",
        "&" => "&",
        "^" => "^",
        _ => return None,
    })
}

// Fixtures + specs ----------------------------------------------------

/// Emit one fixture file as `spec/fixtures/<table>.ts` — a set of
/// named exports, one per fixture record. IDs assigned in insertion
/// order (1..N), matching Rust/Crystal emit.
fn emit_ts_fixture(lowered: &crate::lower::LoweredFixture) -> EmittedFile {
    let fixture_name = lowered.name.as_str();
    let class_name = lowered.class.0.as_str();
    let model_file = crate::naming::snake_case(class_name);

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(
        s,
        "import {{ {class_name} }} from \"../../app/models/{model_file}.js\";"
    )
    .unwrap();
    writeln!(s, "import {{ FIXTURE_IDS, fixtureId }} from \"../fixtures.js\";").unwrap();

    // `_loadAll` — called from Fixtures.setup inside beforeEach. Each
    // record runs through the model's `save` getter (validations +
    // INSERT via the juntos runtime); the AUTOINCREMENT id lands in
    // FIXTURE_IDS so the named getters and cross-fixture FK refs can
    // find it.
    writeln!(s).unwrap();
    writeln!(s, "export function _loadAll(): void {{").unwrap();
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s, "  {{").unwrap();
        writeln!(s, "    const record = new {class_name}();").unwrap();
        for field in &record.fields {
            let col = field.column.as_str();
            let val = match &field.value {
                crate::lower::LoweredFixtureValue::Literal { ty, raw } => {
                    ts_literal_for(raw, ty)
                }
                crate::lower::LoweredFixtureValue::FkLookup {
                    target_fixture,
                    target_label,
                } => format!(
                    "fixtureId({:?}, {:?})",
                    target_fixture.as_str(),
                    target_label.as_str(),
                ),
            };
            writeln!(s, "    record.{col} = {val};").unwrap();
        }
        writeln!(
            s,
            "    if (!record.save) throw new Error(\"fixture {fixture_name}/{label} failed to save\");",
        )
        .unwrap();
        writeln!(
            s,
            "    FIXTURE_IDS.set(\"{fixture_name}:{label}\", record.id);",
        )
        .unwrap();
        writeln!(s, "  }}").unwrap();
    }
    writeln!(s, "}}").unwrap();

    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s).unwrap();
        writeln!(
            s,
            "export function {label}(): {class_name} {{",
        )
        .unwrap();
        writeln!(
            s,
            "  const id = fixtureId({fixture_name:?}, {label:?});"
        )
        .unwrap();
        writeln!(
            s,
            "  const record = {class_name}.find(id);"
        )
        .unwrap();
        writeln!(
            s,
            "  if (!record) throw new Error(\"fixture {fixture_name}/{label} not loaded\");",
        )
        .unwrap();
        writeln!(s, "  return record;").unwrap();
        writeln!(s, "}}").unwrap();
    }

    EmittedFile {
        path: PathBuf::from(format!("spec/fixtures/{fixture_name}.ts")),
        content: s,
    }
}

/// `spec/fixtures.ts` — top-level fixture harness: the shared
/// FIXTURE_IDS map, the `setup()` entrypoint that every spec's
/// `beforeEach` runs, and the `fixtureId` lookup helper.
fn emit_ts_fixtures_helper(lowered: &crate::lower::LoweredFixtureSet) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "import {{ setupTestDb }} from \"juntos\";").unwrap();
    writeln!(s, "import {{ CREATE_TABLES }} from \"../src/schema_sql.js\";").unwrap();
    for f in &lowered.fixtures {
        writeln!(
            s,
            "import * as {}Fixtures from \"./fixtures/{}.js\";",
            crate::naming::camelize(f.name.as_str()),
            f.name.as_str(),
        )
        .unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "export const FIXTURE_IDS = new Map<string, number>();").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "/** Per-test entry point. Opens a fresh :memory: DB, runs schema").unwrap();
    writeln!(s, " *  DDL, and reloads every fixture in declaration order. */").unwrap();
    writeln!(s, "export function setup(): void {{").unwrap();
    writeln!(s, "  setupTestDb(CREATE_TABLES);").unwrap();
    writeln!(s, "  FIXTURE_IDS.clear();").unwrap();
    for f in &lowered.fixtures {
        writeln!(
            s,
            "  {}Fixtures._loadAll();",
            crate::naming::camelize(f.name.as_str())
        )
        .unwrap();
    }
    writeln!(s, "}}").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "export function fixtureId(fixture: string, label: string): number {{").unwrap();
    writeln!(s, "  const v = FIXTURE_IDS.get(`${{fixture}}:${{label}}`);").unwrap();
    writeln!(
        s,
        "  if (v === undefined) throw new Error(`fixture ${{fixture}}/${{label}} not loaded`);"
    )
    .unwrap();
    writeln!(s, "  return v;").unwrap();
    writeln!(s, "}}").unwrap();
    EmittedFile {
        path: PathBuf::from("spec/fixtures.ts"),
        content: s,
    }
}

/// Map a Roundhouse `Ty` to (type annotation, default expression) for
/// a TS instance field declaration. Keeping both Time and DateTime as
/// `string` matches the Rust/Crystal convention — SQLite stores ISO
/// strings; generated tests compare them as opaque values.
pub(super) fn ts_field_type_and_default(ty: &Ty) -> (String, String) {
    match ty {
        Ty::Int => ("number".into(), "0".into()),
        Ty::Float => ("number".into(), "0".into()),
        Ty::Bool => ("boolean".into(), "false".into()),
        Ty::Str | Ty::Sym => ("string".into(), "\"\"".into()),
        Ty::Class { id, .. } if id.0.as_str() == "Time" => ("string".into(), "\"\"".into()),
        _ => ("any".into(), "null".into()),
    }
}

fn ts_literal_for(value: &str, ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                value.to_string()
            } else {
                format!("0 /* TODO: coerce {value:?} */")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                value.to_string()
            } else {
                format!("0 /* TODO: coerce {value:?} */")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "true".into(),
            "false" | "0" => "false".into(),
            _ => format!("false /* TODO: coerce {value:?} */"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}

fn emit_ts_spec(tm: &TestModule, app: &App) -> EmittedFile {
    let fixture_names: Vec<Symbol> =
        app.fixtures.iter().map(|f| f.name.clone()).collect();
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let mut attrs_set: std::collections::BTreeSet<Symbol> =
        std::collections::BTreeSet::new();
    for m in &app.models {
        for attr in m.attributes.fields.keys() {
            attrs_set.insert(attr.clone());
        }
    }
    let model_attrs: Vec<Symbol> = attrs_set.into_iter().collect();

    let ctx = SpecCtx {
        app,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
    };

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "import {{ test, beforeEach }} from \"node:test\";").unwrap();
    writeln!(s, "import assert from \"node:assert/strict\";").unwrap();
    if is_controller_test {
        // Side-effect import: routes.ts pushes into the Router match
        // table at module load. Needed before any dispatch.
        writeln!(s, "import \"../../src/routes.js\";").unwrap();
        writeln!(s, "import {{ TestClient }} from \"../../src/test_support.js\";").unwrap();
        writeln!(s, "import * as routeHelpers from \"../../src/route_helpers.js\";").unwrap();
    }
    // Per-model class imports — one per known model, so `new Article()`
    // etc. in test bodies resolve. Models are siblings under
    // app/models/.
    for model_name in &known_models {
        let file = crate::naming::snake_case(model_name.as_str());
        writeln!(
            s,
            "import {{ {} }} from \"../../app/models/{}.js\";",
            model_name, file,
        )
        .unwrap();
    }
    for fixture in &app.fixtures {
        writeln!(
            s,
            "import * as {}Fixtures from \"../fixtures/{}.js\";",
            crate::naming::camelize(fixture.name.as_str()),
            fixture.name,
        )
        .unwrap();
    }
    // Every test starts on a fresh :memory: DB + fully-loaded
    // fixtures. `beforeEach` runs across the whole spec file, matching
    // Rails' transactional-fixture isolation.
    if !app.fixtures.is_empty() {
        writeln!(s, "import {{ setup }} from \"../fixtures.js\";").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "beforeEach(() => setup());").unwrap();
    }

    for test in &tm.tests {
        writeln!(s).unwrap();
        let test_name = &test.name;
        if is_controller_test {
            emit_ts_controller_test(&mut s, test, app);
        } else if test_needs_runtime_unsupported_ts(test) {
            writeln!(
                s,
                "test.skip({test_name:?}, () => {{"
            )
            .unwrap();
            writeln!(s, "  // Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "}});").unwrap();
        } else {
            writeln!(s, "test({test_name:?}, () => {{").unwrap();
            let body = emit_spec_body_ts(&test.body, ctx);
            for line in body.lines() {
                writeln!(s, "  {line}").unwrap();
            }
            writeln!(s, "}});").unwrap();
        }
    }

    let filename = crate::naming::snake_case(tm.name.0.as_str());
    let filename = filename.replace("_test", ".test");
    EmittedFile {
        path: PathBuf::from(format!("spec/models/{filename}.ts")),
        content: s,
    }
}

#[derive(Clone, Copy)]
struct SpecCtx<'a> {
    app: &'a App,
    fixture_names: &'a [Symbol],
    known_models: &'a [Symbol],
    model_attrs: &'a [Symbol],
}

fn emit_spec_body_ts(body: &Expr, ctx: SpecCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|e| emit_spec_stmt_ts(e, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_spec_stmt_ts(body, ctx),
    }
}

fn emit_spec_stmt_ts(e: &Expr, ctx: SpecCtx) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("const {} = {};", name, emit_spec_expr_ts(value, ctx))
        }
        _ => format!("{};", emit_spec_expr_ts(e, ctx)),
    }
}

fn emit_spec_expr_ts(e: &Expr, ctx: SpecCtx) -> String {
    use crate::expr::InterpPart;
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    let key = match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            value.as_str().to_string()
                        }
                        _ => emit_spec_expr_ts(k, ctx),
                    };
                    format!("{}: {}", key, emit_spec_expr_ts(v, ctx))
                })
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> =
                elements.iter().map(|e| emit_spec_expr_ts(e, ctx)).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => out.push_str(value),
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_spec_expr_ts(expr, ctx));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_spec_send_ts(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!(
                "{} {} {}",
                emit_spec_expr_ts(left, ctx),
                op_s,
                emit_spec_expr_ts(right, ctx)
            )
        }
        _ => format!("/* TODO: spec emit for {:?} */", std::mem::discriminant(&*e.node)),
    }
}

fn emit_spec_send_ts(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: SpecCtx,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_spec_expr_ts(a, ctx)).collect();

    // Fixture accessor: articles(:one) → articlesFixtures.one()
    if recv.is_none()
        && args.len() == 1
        && ctx.fixture_names.iter().any(|s| s.as_str() == method)
    {
        if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*args[0].node {
            let ns = crate::naming::camelize(method);
            return format!("{ns}Fixtures.{}()", sym.as_str());
        }
    }

    // assert_difference("Class.count", delta) do ... end → inline
    // before/after capture + assert.strictEqual on the delta.
    if recv.is_none() && method == "assert_difference" {
        if let Some(body) = block {
            if let Some(count_expr) = args
                .first()
                .and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => {
                        rewrite_ruby_dot_call_ts(value)
                    }
                    _ => None,
                })
            {
                let delta = args_s.get(1).cloned().unwrap_or_else(|| "1".into());
                let body_s = emit_block_body_ts(body, ctx);
                return format!(
                    "(() => {{\n  const _before = {count_expr};\n  {body_s}\n  const _after = {count_expr};\n  assert.strictEqual(_after - _before, {delta});\n}})()"
                );
            }
        }
    }

    // owner.<assoc>.create(hash) / .build(hash) — HasMany rewrite.
    if (method == "create" || method == "build") && args.len() == 1 {
        if let Some(outer_recv) = recv {
            if let ExprNode::Send {
                recv: Some(assoc_recv),
                method: assoc_method,
                args: inner_args,
                ..
            } = &*outer_recv.node
            {
                if inner_args.is_empty() {
                    if let Some(s) = try_emit_assoc_create_ts(
                        assoc_recv,
                        assoc_method.as_str(),
                        args,
                        method,
                        ctx,
                    ) {
                        return s;
                    }
                }
            }
        }
    }

    // Assertion mapping. node:test uses `node:assert/strict` which
    // provides `assert.strictEqual`, `assert.notStrictEqual`,
    // `assert.ok`, etc. Ruby's `assert_equal expected, actual`
    // argument order flips to actual-first for Node.
    if recv.is_none() {
        match (method, args_s.len()) {
            ("assert_equal", 2) => {
                return format!("assert.strictEqual({}, {})", args_s[1], args_s[0]);
            }
            ("assert_not_equal", 2) => {
                return format!("assert.notStrictEqual({}, {})", args_s[1], args_s[0]);
            }
            ("assert_not", 1) => {
                return format!("assert.ok(!({}))", args_s[0]);
            }
            ("assert", 1) => {
                return format!("assert.ok({})", args_s[0]);
            }
            ("assert_nil", 1) => {
                return format!("assert.strictEqual({}, null)", args_s[0]);
            }
            ("assert_not_nil", 1) => {
                return format!("assert.notStrictEqual({}, null)", args_s[0]);
            }
            _ => {}
        }
    }

    // `Class.new(hash)` → Object.assign(new Class(), { k: v, ... })
    if let Some(r) = recv {
        if method == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class_name) = path.last() {
                    if ctx.known_models.iter().any(|s| s == class_name) {
                        if let ExprNode::Hash { entries, .. } = &*args[0].node {
                            let pairs: Vec<String> = entries
                                .iter()
                                .map(|(k, v)| {
                                    let key = match &*k.node {
                                        ExprNode::Lit {
                                            value: Literal::Sym { value: f },
                                        } => f.as_str().to_string(),
                                        _ => emit_spec_expr_ts(k, ctx),
                                    };
                                    format!("{}: {}", key, emit_spec_expr_ts(v, ctx))
                                })
                                .collect();
                            return format!(
                                "Object.assign(new {}(), {{ {} }})",
                                class_name,
                                pairs.join(", ")
                            );
                        }
                    }
                }
            }
        }
    }

    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_spec_expr_ts(r, ctx);
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            // Class-level calls are always methods (e.g. `Comment.count`).
            if is_class_call {
                if args_s.is_empty() {
                    return format!("{recv_s}.{method}()");
                } else {
                    return format!("{recv_s}.{method}({})", args_s.join(", "));
                }
            }
            // Instance-level calls: attribute reads + Juntos getters
            // (save, destroy) render without parens; other zero-arg
            // calls go method-style.
            let is_attr_read = args_s.is_empty()
                && ctx.model_attrs.iter().any(|s| s.as_str() == method);
            let is_juntos_getter =
                args_s.is_empty() && matches!(method, "save" | "destroy");
            if is_attr_read || is_juntos_getter {
                format!("{recv_s}.{method}")
            } else if args_s.is_empty() {
                format!("{recv_s}.{method}()")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

/// Parse a Ruby-style `"Class.method"` expression string into TS
/// `Class.method()` syntax. Used by `assert_difference` to re-
/// evaluate the captured count expression before and after the block.
fn rewrite_ruby_dot_call_ts(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    let (lhs, rhs) = trimmed.split_once('.')?;
    let is_ident = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if !is_ident(lhs) || !is_ident(rhs) {
        return None;
    }
    // Uppercase LHS → class-level call (`Comment.count()`).
    // Lowercase LHS → instance attribute read (`article.count`).
    let is_class = lhs.chars().next().is_some_and(|c| c.is_uppercase());
    if is_class {
        Some(format!("{lhs}.{rhs}()"))
    } else {
        Some(format!("{lhs}.{rhs}"))
    }
}

/// Render a Ruby block body as TS statements, peeling one Lambda
/// layer. Ruby `do ... end` lowers to `ExprNode::Lambda`.
fn emit_block_body_ts(e: &Expr, ctx: SpecCtx) -> String {
    let inner = match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    };
    match &*inner.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|s| emit_spec_stmt_ts(s, ctx))
            .collect::<Vec<_>>()
            .join("\n  "),
        _ => emit_spec_stmt_ts(inner, ctx),
    }
}

fn try_emit_assoc_create_ts(
    owner: &Expr,
    assoc_name: &str,
    args: &[Expr],
    outer_method: &str,
    ctx: SpecCtx,
) -> Option<String> {
    let resolved = crate::lower::resolve_has_many(
        &Symbol::from(assoc_name),
        owner.ty.as_ref(),
        ctx.app,
    )?;
    let target_class = resolved.target_class.0.as_str();
    let foreign_key = resolved.foreign_key.as_str();

    let owner_s = emit_spec_expr_ts(owner, ctx);
    let hash_entries = match &args.first()?.node.as_ref() {
        ExprNode::Hash { entries, .. } => entries.clone(),
        _ => return None,
    };

    let mut pairs: Vec<String> =
        vec![format!("{foreign_key}: {owner_s}.id")];
    for (k, v) in &hash_entries {
        if let ExprNode::Lit { value: Literal::Sym { value: field_name } } = &*k.node {
            pairs.push(format!("{}: {}", field_name.as_str(), emit_spec_expr_ts(v, ctx)));
        }
    }
    // `.build` returns the unsaved record; `.create` runs save first.
    // Both evaluate to the record.
    let construct = format!(
        "Object.assign(new {target_class}(), {{ {} }})",
        pairs.join(", "),
    );
    if outer_method == "create" {
        Some(format!("(() => {{ const _r = {construct}; _r.save; return _r; }})()"))
    } else {
        Some(construct)
    }
}

fn test_needs_runtime_unsupported_ts(_test: &Test) -> bool {
    // Phase 3 rounded out the TS emitter's handling of the real-blog
    // test shapes. Keep as future-guard; no current pattern forces a
    // skip.
    false
}

#[allow(dead_code)]

fn test_body_uses_unsupported_ts(e: &Expr) -> bool {
    use crate::expr::InterpPart;
    let self_hit = match &*e.node {
        ExprNode::Send { recv, method, .. } => {
            let m = method.as_str();
            matches!(
                m,
                "assert_difference"
                    | "destroy"
                    | "destroy!"
                    | "build"
                    | "create"
                    | "create!"
            ) || (m == "count"
                && recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. })))
        }
        _ => false,
    };
    if self_hit {
        return true;
    }
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if test_body_uses_unsupported_ts(r) {
                    return true;
                }
            }
            for a in args {
                if test_body_uses_unsupported_ts(a) {
                    return true;
                }
            }
            if let Some(b) = block {
                if test_body_uses_unsupported_ts(b) {
                    return true;
                }
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                if test_body_uses_unsupported_ts(e) {
                    return true;
                }
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                if test_body_uses_unsupported_ts(k) || test_body_uses_unsupported_ts(v) {
                    return true;
                }
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    if test_body_uses_unsupported_ts(expr) {
                        return true;
                    }
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            if test_body_uses_unsupported_ts(left) || test_body_uses_unsupported_ts(right) {
                return true;
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            if test_body_uses_unsupported_ts(cond)
                || test_body_uses_unsupported_ts(then_branch)
                || test_body_uses_unsupported_ts(else_branch)
            {
                return true;
            }
        }
        ExprNode::Let { value, body, .. } => {
            if test_body_uses_unsupported_ts(value) || test_body_uses_unsupported_ts(body) {
                return true;
            }
        }
        ExprNode::Lambda { body, .. } => {
            if test_body_uses_unsupported_ts(body) {
                return true;
            }
        }
        ExprNode::Assign { value, .. } => {
            if test_body_uses_unsupported_ts(value) {
                return true;
            }
        }
        _ => {}
    }
    false
}

/// Emit a single controller test (`test("name", async () => { ... })`).
/// Mirrors `emit_rust_controller_test`. Primes ivars from fixtures,
/// opens a fresh `TestClient`, then walks test-body statements.
fn emit_ts_controller_test(out: &mut String, test: &Test, app: &App) {
    writeln!(out, "test({:?}, async () => {{", test.name.as_str()).unwrap();
    writeln!(out, "  const client = new TestClient();").unwrap();

    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(&crate::naming::camelize(ivar.as_str()));
        let fixt_ns = crate::naming::camelize(&plural);
        writeln!(
            out,
            "  const {} = {}Fixtures.one();",
            ivar.as_str(),
            fixt_ns,
        )
        .unwrap();
    }

    let stmts = ts_ctrl_test_body_stmts(&test.body);
    for stmt in stmts {
        let rendered = emit_ts_ctrl_test_stmt(stmt, app);
        for line in rendered.lines() {
            writeln!(out, "  {line}").unwrap();
        }
    }

    writeln!(out, "}});").unwrap();
}

fn ts_ctrl_test_body_stmts(body: &Expr) -> Vec<&Expr> {
    crate::lower::test_body_stmts(body)
}

fn emit_ts_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ts_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ts_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.reload();");
            }
            let recv_s = emit_ts_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{recv_s}.{method};")
            } else {
                format!("{recv_s}.{method}({});", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("let {name} = {};", emit_ts_ctrl_test_expr(value, app))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("let {name} = {};", emit_ts_ctrl_test_expr(value, app))
        }
        _ => format!("{};", emit_ts_ctrl_test_expr(stmt, app)),
    }
}

fn emit_ts_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_ts_url_expr(url, app);
            format!("const resp = await client.get({u});")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_ts_url_expr(url, app);
            let body = params
                .map(|h| flatten_ts_params_to_form(h, None, app))
                .unwrap_or_else(|| "{}".to_string());
            format!("const resp = await client.{method}({u}, {body});")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_ts_url_expr(url, app);
            format!("const resp = await client.delete({u});")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assertOk();".to_string(),
            "unprocessable_entity" => "resp.assertUnprocessable();".to_string(),
            other => format!("resp.assertStatus(/* {other:?} */ 200);"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_ts_url_expr(url, app);
            format!("resp.assertRedirectedTo({u});")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_ts_assert_select_classified(selector, kind, app)
        }
        Some(ControllerTestSend::AssertDifference { method, count_expr, delta, block }) => {
            let _ = method;
            emit_ts_assert_difference_classified(count_expr, delta, block, app)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_ts_ctrl_test_expr(expected, app);
            let a = emit_ts_ctrl_test_expr(actual, app);
            // Rails calls assert_equal(expected, actual); node:test
            // assert.equal takes (actual, expected) — swap order.
            format!("assert.equal({a}, {e});")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{method}();")
            } else {
                format!("{method}({});", args_s.join(", "))
            }
        }
    }
}

/// `articles_url`, `article_url(@article)` → `routeHelpers.articlesPath()`,
/// `routeHelpers.articlePath(article.id)`. Uses the shared URL-helper
/// classifier; target-specific pieces are (a) helper name casing and
/// (b) the optional-unwrap syntax for `Model.last`.
fn emit_ts_url_expr(expr: &Expr, app: &App) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_ts_ctrl_test_expr(expr, app);
    };
    let camel = crate::naming::camelize(&helper.helper_base);
    let mut chars = camel.chars();
    let helper_name = match chars.next() {
        Some(c) => format!("{}{}Path", c.to_lowercase(), chars.as_str()),
        None => format!("{}Path", helper.helper_base),
    };
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last()!.id", class.as_str()),
            UrlArg::Raw(e) => emit_ts_ctrl_test_expr(e, app),
        })
        .collect();
    format!("routeHelpers.{helper_name}({})", args_s.join(", "))
}

fn emit_ts_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } = &*selector_expr.node
    else {
        return format!(
            "/* TODO: dynamic selector */ resp.assertSelect({});",
            emit_ts_ctrl_test_expr(selector_expr, app),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_ts_ctrl_test_expr(expr, app);
            format!("resp.assertSelectText({selector:?}, {text});")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_ts_ctrl_test_expr(expr, app);
            format!("resp.assertSelectMin({selector:?}, {n});")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("resp.assertSelect({selector:?});\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in ts_ctrl_test_body_stmts(inner_body) {
                out.push_str(&emit_ts_ctrl_test_stmt(stmt, app));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assertSelect({selector:?});")
        }
    }
}

fn emit_ts_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    // `Article.count` → `Article.count()`.
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}.{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("const _before = {count_expr};\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in ts_ctrl_test_body_stmts(inner_body) {
            out.push_str(&emit_ts_ctrl_test_stmt(stmt, app));
            out.push('\n');
        }
    }
    out.push_str(&format!("const _after = {count_expr};\n"));
    out.push_str(&format!("assert.equal(_after - _before, {expected_delta});"));
    out
}

fn emit_ts_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_ts_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last()!");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count()");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ts_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_ts_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_ts_url_expr(expr, app);
            }
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("/* TODO expr {:?} */", std::mem::discriminant(&*expr.node)),
    }
}

fn emit_ts_literal(v: &Literal) -> String {
    match v {
        Literal::Str { value } => format!("{value:?}"),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => value.to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Nil => "null".to_string(),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
    }
}

/// Flatten `{ article: { title: "X", body: "Y" } }` into a TS object
/// literal `{ "article[title]": "X", "article[body]": "Y" }` matching
/// the TestClient's form-body shape (which the controller template
/// destructures via `context.params["article[title]"]`). The key
/// flattening lives in `crate::lower::flatten_params_pairs`; this
/// function is just the TS-side value render.
fn flatten_ts_params_to_form(expr: &Expr, scope: Option<&str>, app: &App) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_ts_ctrl_test_expr(value, app);
            format!("{key:?}: String({val})")
        })
        .collect();
    format!("{{ {} }}", pairs.join(", "))
}
