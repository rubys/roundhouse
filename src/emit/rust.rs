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
use crate::dialect::{HttpMethod, Test, TestModule};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::Symbol;
use crate::naming::snake_case;
use crate::ty::Ty;

mod controller;
mod model;
mod view;

use controller::{emit_body, EmitCtx};

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

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();

    // Project skeleton: Cargo.toml + src/lib.rs. These tag along
    // unconditionally so the output is a self-contained Cargo project
    // the target toolchain will accept.
    files.push(emit_cargo_toml());

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
        files.push(emit_schema_sql(app));
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
        files.push(emit_main_rs(app));
        files.push(emit_rust_importmap(app));
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
        files.push(emit_router(app));
        // Route helper functions (`articles_path()`, `article_path(
        // id)`, …) emitted from the route table.
        files.push(emit_route_helpers(app));
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
            files.push(emit_rust_fixture(f));
        }
        files.push(emit_fixtures_mod(&lowered));
    }

    // Tests — one Rust test module per Ruby test file.
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(emit_rust_test_module(tm, app));
        }
        files.push(emit_tests_mod(&app.test_modules));
    }

    // lib.rs declares the modules we emitted.
    files.push(emit_lib_rs(app));

    files
}

/// Cargo.toml for the generated crate. Includes axum (with ws for
/// Action Cable) for the HTTP runtime, serde + serde_json for typed
/// form decoding and JSON frame encoding, tokio for the async
/// runtime axum depends on, rusqlite for persistence, futures-util
/// for the WebSocket stream combinators, and axum-test (dev-only)
/// for the controller test client.
fn emit_cargo_toml() -> EmittedFile {
    let content = "\
[package]
name = \"app\"
version = \"0.1.0\"
edition = \"2024\"

[lib]
path = \"src/lib.rs\"

[[bin]]
name = \"app\"
path = \"src/main.rs\"

[dependencies]
axum = { version = \"0.8\", features = [\"ws\"] }
base64 = \"0.22\"
futures-util = \"0.3\"
rusqlite = { version = \"0.33\", features = [\"bundled\"] }
serde = { version = \"1\", features = [\"derive\"] }
serde_json = \"1\"
tokio = { version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"net\", \"sync\", \"time\"] }

[dev-dependencies]
axum-test = \"18\"
";
    EmittedFile {
        path: PathBuf::from("Cargo.toml"),
        content: content.to_string(),
    }
}

/// `src/lib.rs` — declares the crate modules Roundhouse emitted.
fn emit_lib_rs(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    if !app.models.is_empty() {
        writeln!(s, "pub mod runtime;").unwrap();
        writeln!(s, "pub mod db;").unwrap();
        writeln!(s, "pub mod schema_sql;").unwrap();
        writeln!(s, "pub mod models;").unwrap();
    }
    if !app.controllers.is_empty() {
        writeln!(s, "pub mod http;").unwrap();
        writeln!(s, "pub mod controllers;").unwrap();
        writeln!(s, "pub mod router;").unwrap();
        writeln!(s, "pub mod route_helpers;").unwrap();
        writeln!(s, "pub mod importmap;").unwrap();
        writeln!(s, "pub mod view_helpers;").unwrap();
        writeln!(s, "pub mod views;").unwrap();
        writeln!(s, "pub mod server;").unwrap();
        writeln!(s, "pub mod cable;").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "#[cfg(test)]").unwrap();
        writeln!(s, "pub mod test_support;").unwrap();
    }
    if !app.fixtures.is_empty() {
        writeln!(s).unwrap();
        writeln!(s, "#[cfg(test)]").unwrap();
        writeln!(s, "pub mod fixtures;").unwrap();
    }
    if !app.test_modules.is_empty() {
        writeln!(s).unwrap();
        writeln!(s, "#[cfg(test)]").unwrap();
        writeln!(s, "pub mod tests;").unwrap();
    }
    EmittedFile {
        path: PathBuf::from("src/lib.rs"),
        content: s,
    }
}

/// `src/main.rs` — server entry point. Opens the DB, applies the
/// schema, and starts axum with the generated router. Uses
/// `#[tokio::main]` so tokio's multi-thread runtime bootstraps
/// before calling into `server::start`, which needs the runtime
/// active to bind the listener.
/// Emit `src/importmap.rs` — the app's ingested importmap pins
/// as a static `PINS` slice. The layout's `javascript_importmap_
/// tags` helper call passes this slice in. Mirrors the TS
/// target's `src/importmap.ts` emit.
fn emit_rust_importmap(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "pub static PINS: &[(&str, &str)] = &["
    )
    .unwrap();
    if let Some(importmap) = &app.importmap {
        for pin in &importmap.pins {
            writeln!(s, "    ({:?}, {:?}),", pin.name, pin.path).unwrap();
        }
    }
    writeln!(s, "];").unwrap();
    EmittedFile {
        path: PathBuf::from("src/importmap.rs"),
        content: s,
    }
}

/// Compose one AR modifier onto the running rust expression.
/// `all`/`includes`/`preload`/`joins`/`distinct`/`select` are
/// no-ops for our in-memory Vec. `order({field: :dir})` lowers to
/// a `.sort_by` with a direction-aware comparator. `limit(N)`
/// truncates via `.into_iter().take(N).collect()`.
///
/// Chain-walk lives in `src/lower/chain.rs`; this fn just renders
/// one already-classified layer.
pub(super) fn apply_rust_chain_modifier(prev: String, m: crate::lower::ChainModifier<'_>) -> String {
    match m.method {
        "all" | "includes" | "preload" | "joins" | "distinct" | "select" => prev,
        "order" => {
            let Some(hash) = m.args.first() else { return prev };
            let ExprNode::Hash { entries, .. } = &*hash.node else { return prev };
            let Some((k, v)) = entries.first() else { return prev };
            let field = match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => return prev,
            };
            let dir = match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => "asc".to_string(),
            };
            let cmp = if dir == "desc" {
                format!("|a, b| b.{field}.cmp(&a.{field})")
            } else {
                format!("|a, b| a.{field}.cmp(&b.{field})")
            };
            format!(
                "{{ let mut __v = {prev}; __v.sort_by({cmp}); __v }}"
            )
        }
        "limit" => {
            let Some(n) = m.args.first() else { return prev };
            if let ExprNode::Lit { value: Literal::Int { value } } = &*n.node {
                return format!("{prev}.into_iter().take({value} as usize).collect::<Vec<_>>()");
            }
            prev
        }
        _ => prev,
    }
}

fn emit_main_rs(app: &App) -> EmittedFile {
    let has_app_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application");
    let layout_import = if has_app_layout {
        "use app::views::layouts_application;\n"
    } else {
        ""
    };
    let layout_field = if has_app_layout {
        "            layout: Some(|| layouts_application(&())),\n"
    } else {
        "            layout: None,\n"
    };

    // Register a partial renderer for each model that broadcasts.
    // `crate::cable::broadcast_{prepend,append,replace}_to` calls
    // `render_partial(type_name, id)` internally; we connect it
    // here to the model's actual view partial. Models without
    // broadcasts skip registration — their records never trigger
    // a partial render through the cable path.
    let mut partial_registrations = String::new();
    for model in &app.models {
        if crate::lower::lower_broadcasts(model).is_empty() {
            continue;
        }
        let class = model.name.0.as_str();
        let singular = snake_case(class);
        writeln!(
            partial_registrations,
            "    app::cable::register_partial({class:?}, |id| {{\n        match app::models::{class}::find(id) {{\n            Some(r) => app::views::render_{singular}(&r),\n            None => String::new(),\n        }}\n    }});",
        )
        .unwrap();
    }
    let partial_section = if partial_registrations.is_empty() {
        String::new()
    } else {
        format!("\n{partial_registrations}")
    };

    let content = format!(
        "// Generated by Roundhouse.

use app::{{router, schema_sql, server}};
{layout_import}
#[tokio::main]
async fn main() {{
    let db_path = std::env::var(\"DATABASE_PATH\").ok();
    let port = std::env::var(\"PORT\").ok().and_then(|s| s.parse().ok());
{partial_section}    server::start(
        router::router(),
        server::StartOptions {{
            db_path,
            port,
            schema_sql: schema_sql::CREATE_TABLES,
{layout_field}        }},
    )
    .await;
}}
"
    );
    EmittedFile {
        path: PathBuf::from("src/main.rs"),
        content,
    }
}

// Routes ----------------------------------------------------------------
//
// Phase 4d: emit `src/router.rs` with `pub fn router() -> Router`
// wiring the route table to the controller action fns, plus
// `src/route_helpers.rs` with one `pub fn` per route that takes the
// route's typed path params and returns a String (used by both the
// emitted controller actions for redirects and the emitted tests for
// URLs).

use crate::lower::FlatRoute;

/// Emit `src/router.rs` — `pub fn router() -> Router` wiring the
/// flat route table to controller action fns. Groups routes by path
/// so axum's MethodRouter chain (`.get(...).post(...)`) handles
/// multi-verb endpoints correctly.
fn emit_router(app: &App) -> EmittedFile {
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use axum::Router;").unwrap();
    writeln!(s, "use axum::routing::{{delete, get, patch, post}};").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use crate::controllers;").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "pub fn router() -> Router {{").unwrap();
    writeln!(s, "    Router::new()").unwrap();

    // Group by axum path (translated from Rails' `:id` to axum's
    // `{id}`) so we can stack MethodRouters per path.
    use std::collections::BTreeMap;
    let mut by_path: BTreeMap<String, Vec<&FlatRoute>> = BTreeMap::new();
    for route in &flat {
        by_path
            .entry(to_axum_path(&route.path))
            .or_default()
            .push(route);
    }
    for (path, routes) in &by_path {
        let verbs: Vec<String> = routes
            .iter()
            .map(|r| {
                let handler_path = controller_module_path(r.controller.0.as_str());
                let verb = axum_verb_fn(&r.method);
                format!("{verb}({handler_path}::{})", r.action)
            })
            .collect();
        writeln!(s, "        .route(\"{path}\", {})", verbs.join(".")).unwrap();
    }
    writeln!(s, "}}").unwrap();
    EmittedFile {
        path: PathBuf::from("src/router.rs"),
        content: s,
    }
}

/// Translate `/articles/:id` → `/articles/{id}` for axum 0.8.
fn to_axum_path(rails_path: &str) -> String {
    let mut out = String::new();
    let mut chars = rails_path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' {
            let mut ident = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '_' {
                    ident.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push('{');
            out.push_str(&ident);
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

fn axum_verb_fn(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        _ => "get",
    }
}

fn controller_module_path(class: &str) -> String {
    format!("controllers::{}", snake_case(class))
}

/// Emit `src/route_helpers.rs` — one `pub fn <as_name>_path(...)` per
/// flattened route (indexed by `as_name`, which is already unique
/// per path shape). Path params are `i64` by convention (Rails'
/// default integer primary key).
fn emit_route_helpers(app: &App) -> EmittedFile {
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();

    // One entry per unique `as_name`. If two routes share a name
    // (e.g. index + create on the same path), they also share a
    // path so one helper suffices — emit in first-seen order.
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for route in &flat {
        if !seen.insert(route.as_name.clone()) {
            continue;
        }
        let sig_params: Vec<String> = route
            .path_params
            .iter()
            .map(|p| format!("{p}: i64"))
            .collect();
        let sig = sig_params.join(", ");

        // Body: literal path segments + `format!` interpolation
        // when there are params.
        let body = if route.path_params.is_empty() {
            format!("{:?}.to_string()", route.path)
        } else {
            // Rails' `:param` → Rust's `{}` in a format! string.
            let mut fmt = String::new();
            let mut chars = route.path.chars().peekable();
            while let Some(c) = chars.next() {
                if c == ':' {
                    while let Some(&nc) = chars.peek() {
                        if nc.is_alphanumeric() || nc == '_' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    fmt.push_str("{}");
                } else {
                    fmt.push(c);
                }
            }
            format!("format!({fmt:?}, {})", route.path_params.join(", "))
        };

        writeln!(
            s,
            "pub fn {}_path({sig}) -> String {{ {body} }}",
            route.as_name,
        )
        .unwrap();
    }
    EmittedFile {
        path: PathBuf::from("src/route_helpers.rs"),
        content: s,
    }
}

/// Emit `src/schema_sql.rs` — a single `pub const CREATE_TABLES: &str`
/// wrapping the target-neutral DDL produced by `lower::lower_schema`.
/// The Phase 3 test harness executes this string on a fresh `:memory:`
/// connection per test. FK declarations stay at the AR layer (via
/// `belongs_to` existence checks and `dependent: :destroy` cascades in
/// model methods), not as SQLite `FOREIGN KEY` constraints — Rails'
/// `dependent:` is an ActiveRecord callback, mirroring it at the DB
/// layer would drift from Rails semantics.
fn emit_schema_sql(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "pub const CREATE_TABLES: &str = r#\"").unwrap();
    write!(s, "{}", crate::lower::lower_schema(&app.schema)).unwrap();
    writeln!(s, "\"#;").unwrap();
    EmittedFile {
        path: PathBuf::from("src/schema_sql.rs"),
        content: s,
    }
}


// Fixtures ------------------------------------------------------------

/// Emit a single `src/fixtures/<name>.rs` file — one `pub fn <label>()`
/// accessor per fixture record, returning the corresponding model
/// struct. IDs are assigned sequentially from 1 (Rails hashes labels
/// into ints; we assign in insertion order for simplicity).
fn emit_rust_fixture(lowered: &crate::lower::LoweredFixture) -> EmittedFile {
    let fixture_name = lowered.name.as_str();
    let class_name = lowered.class.0.as_str();

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use crate::models::{};", class_name).unwrap();

    // `_load_all` — invoked from `crate::fixtures::setup()` at the top
    // of every test. Inserts each record via the model's `save` (so
    // validations apply), captures the AUTOINCREMENT id, and records
    // it in the shared FIXTURE_IDS map so associated fixtures and
    // test getters can look each record up by label.
    writeln!(s).unwrap();
    writeln!(s, "pub fn _load_all() {{").unwrap();
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s, "    let mut r = {} {{", class_name).unwrap();
        for field in &record.fields {
            let col = field.column.as_str();
            let rust_val = match &field.value {
                crate::lower::LoweredFixtureValue::Literal { ty, raw } => {
                    rust_literal_for(raw, ty)
                }
                crate::lower::LoweredFixtureValue::FkLookup {
                    target_fixture,
                    target_label,
                } => format!(
                    "crate::fixtures::fixture_id({:?}, {:?})",
                    target_fixture.as_str(),
                    target_label.as_str(),
                ),
            };
            writeln!(s, "        {col}: {rust_val},").unwrap();
        }
        writeln!(s, "        ..Default::default()").unwrap();
        writeln!(s, "    }};").unwrap();
        writeln!(
            s,
            "    assert!(r.save(), \"fixture {fixture_name}/{label} failed to save\");",
        )
        .unwrap();
        writeln!(
            s,
            "    crate::fixtures::FIXTURE_IDS.with(|m| {{ m.borrow_mut().insert(({fixture_name:?}, {label:?}), r.id); }});",
        )
        .unwrap();
    }
    writeln!(s, "}}").unwrap();

    // Named-fixture getters — `articles::one()` reads back the record
    // this thread's `_load_all` inserted. A failed `find` means the
    // test forgot to call `crate::fixtures::setup()` or the schema
    // doesn't match the model's field list.
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s).unwrap();
        writeln!(s, "pub fn {label}() -> {class_name} {{").unwrap();
        writeln!(
            s,
            "    let id = crate::fixtures::fixture_id({fixture_name:?}, {label:?});",
        )
        .unwrap();
        writeln!(
            s,
            "    {class_name}::find(id).expect(\"fixture {fixture_name}/{label} not loaded — call crate::fixtures::setup() first\")",
        )
        .unwrap();
        writeln!(s, "}}").unwrap();
    }

    EmittedFile {
        path: PathBuf::from(format!("src/fixtures/{fixture_name}.rs")),
        content: s,
    }
}

/// `src/fixtures/mod.rs` — declares the per-table fixture modules plus
/// the test-harness entry point (`setup`) that every emitted test
/// calls first. Owns the thread-local (fixture_name, label) → DB id
/// map that fixture getters and cross-fixture belongs_to refs read
/// through at runtime.
fn emit_fixtures_mod(lowered: &crate::lower::LoweredFixtureSet) -> EmittedFile {
    let fixtures = &lowered.fixtures;
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use std::cell::RefCell;").unwrap();
    writeln!(s, "use std::collections::HashMap;").unwrap();
    writeln!(s).unwrap();
    for fixture in fixtures {
        writeln!(s, "pub mod {};", fixture.name).unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "thread_local! {{").unwrap();
    writeln!(
        s,
        "    pub static FIXTURE_IDS: RefCell<HashMap<(&'static str, &'static str), i64>> ="
    )
    .unwrap();
    writeln!(s, "        RefCell::new(HashMap::new());").unwrap();
    writeln!(s, "}}").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "/// Per-test entry point. Opens a fresh :memory: SQLite connection,"
    )
    .unwrap();
    writeln!(
        s,
        "/// runs the schema DDL, and loads every fixture in declaration order."
    )
    .unwrap();
    writeln!(
        s,
        "/// Idempotent across repeat calls on the same thread — each call replaces"
    )
    .unwrap();
    writeln!(s, "/// the prior connection, so tests start from a clean slate.").unwrap();
    writeln!(s, "pub fn setup() {{").unwrap();
    writeln!(
        s,
        "    crate::db::setup_test_db(crate::schema_sql::CREATE_TABLES);"
    )
    .unwrap();
    writeln!(s, "    FIXTURE_IDS.with(|m| m.borrow_mut().clear());").unwrap();
    for fixture in fixtures {
        writeln!(s, "    {}::_load_all();", fixture.name).unwrap();
    }
    writeln!(s, "}}").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "pub fn fixture_id(fixture: &'static str, label: &'static str) -> i64 {{").unwrap();
    writeln!(s, "    FIXTURE_IDS.with(|m| {{").unwrap();
    writeln!(s, "        *m.borrow().get(&(fixture, label)).unwrap_or_else(|| {{").unwrap();
    writeln!(
        s,
        "            panic!(\"fixture {{}}:{{}} not loaded\", fixture, label)"
    )
    .unwrap();
    writeln!(s, "        }})").unwrap();
    writeln!(s, "    }})").unwrap();
    writeln!(s, "}}").unwrap();
    EmittedFile {
        path: PathBuf::from("src/fixtures/mod.rs"),
        content: s,
    }
}


/// Render a fixture field value as a Rust literal matching the column's
/// static type. Strings go via `"..".to_string()` to produce `String`
/// instead of `&'static str`; numbers and bools pass through as-is.
fn rust_literal_for(value: &str, ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}.to_string()"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                format!("{value}_i64")
            } else {
                format!("0_i64 /* TODO: coerce {value:?} */")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                format!("{value}_f64")
            } else {
                format!("0.0_f64 /* TODO: coerce {value:?} */")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "true".into(),
            "false" | "0" => "false".into(),
            _ => format!("false /* TODO: coerce {value:?} */"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}.to_string()"),
        _ => format!("{value:?}.to_string()"),
    }
}

// Test modules --------------------------------------------------------

/// Emit a `src/tests/<snake>.rs` file containing one `#[test] fn` per
/// Ruby `test "..."` declaration in the source test module. Test names
/// are snake-cased from the Ruby description string. Bodies are rendered
/// with test-context emit enabled (fixture accessors, assertion mapping,
/// struct-literal `Class.new`).
/// Phase 4d controller-test emit. Walks a Rails Minitest body and
/// renders each statement to the axum-test + TestResponseExt shape.
/// Fully pattern-matched — doesn't reuse the SendKind classifier
/// because test-body shapes (`assert_response`, `assert_select`,
/// `get <url>`, etc.) are distinct from controller-body shapes and
/// not shared with other targets.
///
/// Covers the scaffold blog's assertions:
///   - HTTP verbs: `get` / `post` / `patch` / `delete`
///   - Status: `assert_response :success | :unprocessable_entity`
///   - Redirects: `assert_redirected_to <url>`
///   - Structural: `assert_select <sel>[, text]` + nested block +
///     `minimum: N`
///   - Count: `assert_difference(<expr>[, <delta>]) { body }` +
///     `assert_no_difference`
///   - Equality: `assert_equal a, b`
///   - Model: `Model.last`, `@record.reload`
///
/// Setup (`setup do @article = articles(:one) end`) isn't preserved
/// in the current IR, so ivars read-without-assign get auto-primed
/// from the fixtures' `one` entry. Matches real-blog's convention.
fn emit_rust_controller_test(out: &mut String, test: &Test, app: &App) {
    let name = test_fn_name(&test.name);
    writeln!(out, "#[tokio::test(flavor = \"multi_thread\")]").unwrap();
    writeln!(out, "#[allow(unused_mut, unused_variables)]").unwrap();
    writeln!(out, "async fn {name}() {{").unwrap();
    writeln!(out, "    // {:?}", test.name).unwrap();
    writeln!(out, "    fixtures::setup();").unwrap();
    writeln!(
        out,
        "    let server = axum_test::TestServer::new(crate::router::router()).unwrap();",
    )
    .unwrap();

    // Prime each ivar the body reads but doesn't assign, from the
    // `<plural>::one()` fixture accessor. Same convention as Rails'
    // scaffold `setup` block.
    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(&crate::naming::camelize(ivar.as_str()));
        writeln!(
            out,
            "    let mut {} = fixtures::{}::one();",
            ivar.as_str(),
            plural,
        )
        .unwrap();
    }

    let stmts = ctrl_test_body_stmts(&test.body);
    for stmt in stmts {
        let rendered = emit_ctrl_test_stmt(stmt, app);
        for line in rendered.lines() {
            writeln!(out, "    {line}").unwrap();
        }
    }

    writeln!(out, "}}").unwrap();
}

/// Flatten a test body into a statement sequence. If the body is a
/// single Seq, unwrap it; otherwise return a singleton.
fn ctrl_test_body_stmts(body: &Expr) -> Vec<&Expr> {
    crate::lower::test_body_stmts(body)
}

/// Emit a single controller-test statement.
fn emit_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            // Instance method calls — primarily `@record.reload`.
            if method.as_str() == "reload" {
                // Ivar receivers rendered bare (the ivar priming
                // above bound them as locals).
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } => name.to_string(),
                    ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.reload();");
            }
            let recv_s = emit_ctrl_test_expr(r, app);
            let args_s: Vec<String> = args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{recv_s}.{method}();")
            } else {
                format!("{recv_s}.{method}({});", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("let mut {name} = {};", emit_ctrl_test_expr(value, app))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("let mut {name} = {};", emit_ctrl_test_expr(value, app))
        }
        _ => format!("{};", emit_ctrl_test_expr(stmt, app)),
    }
}

/// Top-level Send dispatcher for test body statements. Recognizes
/// Minitest + Rails test primitives via the shared classifier and
/// renders each variant per Rust's axum_test conventions. Unknown
/// shapes fall back to a best-effort `method(args)` render.
fn emit_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_url_expr(url, app);
            format!("let resp = server.get(&{u}).await;")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_url_expr(url, app);
            let form_body = params
                .map(|h| flatten_params_to_form(h, None, app))
                .unwrap_or_else(|| "std::collections::HashMap::<String, String>::new()".to_string());
            format!("let resp = server.{method}(&{u}).form(&{form_body}).await;")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_url_expr(url, app);
            format!("let resp = server.delete(&{u}).await;")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assert_ok();".to_string(),
            "unprocessable_entity" => "resp.assert_unprocessable();".to_string(),
            other => format!("resp.assert_status(/* {other:?} */ 200);"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_url_expr(url, app);
            format!("resp.assert_redirected_to(&{u});")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_assert_select_classified(selector, kind, app)
        }
        Some(ControllerTestSend::AssertDifference { method, count_expr, delta, block }) => {
            let _ = method;
            emit_assert_difference_classified(count_expr, delta, block, app)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_ctrl_test_expr(expected, app);
            let a = emit_ctrl_test_expr(actual, app);
            // Rails calls assert_equal(expected, actual); match
            // Rust's assert_eq! argument order.
            format!("assert_eq!({e}, {a});")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{method}();")
            } else {
                format!("{method}({});", args_s.join(", "))
            }
        }
    }
}

/// Flatten a Ruby-shape params Hash into a Rust `HashMap<String,
/// String>` literal matching Rails' bracketed-key form. Delegates
/// key-flattening to `crate::lower::flatten_params_pairs`; this
/// function is just the Rust-side value render.
fn flatten_params_to_form(expr: &Expr, scope: Option<&str>, app: &App) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_ctrl_test_expr(value, app);
            format!("({key:?}.to_string(), {val}.to_string())")
        })
        .collect();
    format!(
        "std::collections::HashMap::<String, String>::from([{}])",
        pairs.join(", "),
    )
}

/// Render a URL-helper call (`articles_url`, `article_url(@article)`)
/// into a `route_helpers::*_path(...)` call returning `String`. Uses
/// the shared URL-helper classifier — Rust-specific pieces are the
/// `_path` suffix and the `Model::last().unwrap().id` unwrap syntax.
fn emit_url_expr(expr: &Expr, app: &App) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_ctrl_test_expr(expr, app);
    };
    let helper_name = format!("{}_path", helper.helper_base);
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}::last().unwrap().id", class.as_str()),
            UrlArg::Raw(e) => emit_ctrl_test_expr(e, app),
        })
        .collect();
    format!("route_helpers::{helper_name}({})", args_s.join(", "))
}

/// `assert_select` render over the shared classifier. Rust-specific
/// pieces: `&` borrow on string args, `as usize` cast on the
/// minimum-count arg.
fn emit_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } = &*selector_expr.node
    else {
        return format!(
            "/* TODO: dynamic selector */ resp.assert_select({:?});",
            emit_ctrl_test_expr(selector_expr, app),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_ctrl_test_expr(expr, app);
            format!("resp.assert_select_text({selector:?}, &{text});")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_ctrl_test_expr(expr, app);
            format!("resp.assert_select_min({selector:?}, {n} as usize);")
        }
        // Block form: `assert_select "#articles" do assert_select "h2",
        // minimum: 1 end`. Outer selector check + recurse through the
        // block body as parallel assertions (no nested scoping).
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("resp.assert_select({selector:?});\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in ctrl_test_body_stmts(inner_body) {
                out.push_str(&emit_ctrl_test_stmt(stmt, app));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assert_select({selector:?});")
        }
    }
}

/// `assert_difference(<expr>[, <delta>]) { body }` — render with
/// Rust-specific `Model::count()` syntax. Delta + block come
/// pre-classified.
fn emit_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    // Rewrite "Article.count" → "Article::count()".
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}::{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("let _before = {count_expr};\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in ctrl_test_body_stmts(inner_body) {
            out.push_str(&emit_ctrl_test_stmt(stmt, app));
            out.push('\n');
        }
    }
    out.push_str(&format!("let _after = {count_expr};\n"));
    out.push_str(&format!("assert_eq!(_after - _before, {expected_delta});"));
    out
}

/// Expression-level emit for test bodies — literals, ivar reads, a
/// few targeted call rewrites (`Article.last`, `Article.count`).
/// Doesn't try to be general; unknown shapes fall through to a
/// stringified approximation.
fn emit_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    let _ = app;
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            // `Model.last` → `Model::last().unwrap()`.
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}::last().unwrap()");
                }
            }
            // `Model.count` → `Model::count()`.
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}::count()");
                }
            }
            // Attribute read on ivar/var (`@article.title` →
            // `article.title`).
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_ctrl_test_expr(r, app);
            let args_s: Vec<String> = args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            // Bare fn call — probably a route helper.
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_url_expr(expr, app);
            }
            let args_s: Vec<String> = args.iter().map(|a| emit_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("/* TODO expr {:?} */", std::mem::discriminant(&*expr.node)),
    }
}

fn emit_rust_test_module(tm: &TestModule, app: &App) -> EmittedFile {
    let fixture_names: Vec<Symbol> =
        app.fixtures.iter().map(|f| f.name.clone()).collect();
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    // Flat union of attribute names across every model. Dedup so the
    // slice stays compact; collisions on common names (id, body, etc.)
    // are expected and fine.
    let mut attrs_set: std::collections::BTreeSet<Symbol> =
        std::collections::BTreeSet::new();
    for m in &app.models {
        for attr in m.attributes.fields.keys() {
            attrs_set.insert(attr.clone());
        }
    }
    let model_attrs: Vec<Symbol> = attrs_set.into_iter().collect();

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::fixtures;").unwrap();
    writeln!(s, "#[allow(unused_imports)]").unwrap();
    writeln!(s, "use crate::models::*;").unwrap();
    // Controller-test modules additionally reference route helpers,
    // axum-test, and the test-support assertion trait. Extra imports
    // land conditionally so model tests don't pull in axum deps.
    let is_ctrl_test_header = tm.name.0.as_str().ends_with("ControllerTest");
    if is_ctrl_test_header {
        writeln!(s, "#[allow(unused_imports)]").unwrap();
        writeln!(s, "use crate::route_helpers;").unwrap();
        writeln!(s, "#[allow(unused_imports)]").unwrap();
        writeln!(s, "use crate::test_support::TestResponseExt;").unwrap();
    }

    let ctx = EmitCtx {
        self_methods: &[],
        in_test: true,
        in_controller: false,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
        app: Some(app),
    };

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");
    for test in &tm.tests {
        writeln!(s).unwrap();
        if is_controller_test {
            emit_rust_controller_test(&mut s, test, app);
        } else if test_needs_runtime_unsupported(test) {
            // Body would either fail to compile (destroy/count/
            // assert_difference) or fail at run time (save returning
            // true where a DB check would have made it false).
            // Emit with #[ignore] and a short TODO so the test count
            // stays visible in `cargo test` output.
            writeln!(s, "#[test]").unwrap();
            writeln!(s, "#[ignore] // Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "fn {}() {{", test_fn_name(&test.name)).unwrap();
            writeln!(s, "    // {:?}", test.name).unwrap();
            writeln!(s, "    // TODO: requires save/destroy/aggregate support").unwrap();
            writeln!(s, "}}").unwrap();
        } else {
            writeln!(s, "#[test]").unwrap();
            // Test bodies emit `let mut` uniformly so save/destroy
            // calls on model bindings type-check; this allow-attr
            // silences the resulting unused-mut warnings on bindings
            // that never actually mutate.
            writeln!(s, "#[allow(unused_mut)]").unwrap();
            writeln!(s, "fn {}() {{", test_fn_name(&test.name)).unwrap();
            // Every test starts on a fresh :memory: DB with all
            // fixtures loaded. `setup` is idempotent across repeat
            // calls on the same thread, so a prior test's state
            // never leaks in.
            if !app.fixtures.is_empty() {
                writeln!(s, "    crate::fixtures::setup();").unwrap();
            }
            for line in emit_body(&test.body, ctx).lines() {
                writeln!(s, "    {line}").unwrap();
            }
            writeln!(s, "}}").unwrap();
        }
    }

    let filename = snake_case(tm.name.0.as_str());
    EmittedFile {
        path: PathBuf::from(format!("src/tests/{filename}.rs")),
        content: s,
    }
}

/// `src/tests/mod.rs` — declares the per-file test modules.
fn emit_tests_mod(test_modules: &[TestModule]) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    for tm in test_modules {
        writeln!(s, "pub mod {};", snake_case(tm.name.0.as_str())).unwrap();
    }
    EmittedFile {
        path: PathBuf::from("src/tests/mod.rs"),
        content: s,
    }
}

/// Convert a Ruby test description (`"creates an article with valid
/// attributes"`) to a valid Rust function name. Non-word characters
/// become underscores; leading/trailing underscores stripped.
fn test_fn_name(desc: &str) -> String {
    let mut s: String = desc
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    // Collapse runs of `_`.
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    s.trim_matches('_').to_string()
}

/// Heuristic: does the test body reference runtime support we haven't
/// built yet? Phase 3 brought SQLite-backed persistence, associations,
/// belongs_to existence, dependent destroy, and assert_difference —
/// all previous skip reasons for real-blog now have emit support.
/// Keep the walk as a safety net for any future test body whose shape
/// exceeds what the current emitter handles; real-blog currently
/// triggers none of the remaining cases.
fn test_needs_runtime_unsupported(test: &Test) -> bool {
    test_body_uses_unsupported(&test.body)
}

fn test_body_uses_unsupported(_e: &Expr) -> bool {
    // Phase 3 rounded out the list of Ruby/Rails primitives the Rust
    // emitter handles; no real-blog pattern currently forces a skip.
    // Add shape-specific checks back here if a future fixture demands.
    false
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "None".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => format!("{value}_i64"),
        Literal::Float { value } => format!("{value}_f64"),
        Literal::Str { value } => format!("{value:?}.to_string()"),
        // Ruby symbols map to `String` in our Rust shape (see
        // `rust_ty` for `Ty::Sym`). Emit with the `.to_string()` coercion
        // so Hash entries mixing `"x"` (strings) and `:y` (symbols) stay
        // a uniform `HashMap<&str, String>`.
        Literal::Sym { value } => format!("{:?}.to_string()", value.as_str()),
    }
}

pub fn rust_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "i64".to_string(),
        Ty::Float => "f64".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str => "String".to_string(),
        Ty::Sym => "String".to_string(),
        Ty::Nil => "()".to_string(),
        Ty::Array { elem } => format!("Vec<{}>", rust_ty(elem)),
        Ty::Hash { key, value } => {
            format!("std::collections::HashMap<{}, {}>", rust_ty(key), rust_ty(value))
        }
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(rust_ty).collect();
            format!("({})", parts.join(", "))
        }
        Ty::Record { .. } => "serde_json::Value".to_string(),
        Ty::Union { variants } => option_shape(variants).unwrap_or_else(|| {
            // Non-nullable unions: fall back to a boxed trait object for now.
            // Real answer: emit an enum. Landing when a fixture demands it.
            "Box<dyn std::any::Any>".to_string()
        }),
        Ty::Class { id, .. } => match id.0.as_str() {
            // Schema Date/DateTime/Time columns carry Ty::Class(Time); map
            // to String for now so models emit compilable Rust. A future
            // step with a chrono/time dep can upgrade this to a real
            // DateTime type.
            "Time" => "String".to_string(),
            other => other.to_string(),
        },
        Ty::Fn { .. } => "Box<dyn Fn()>".to_string(),
        Ty::Var { .. } => "()".to_string(),
    }
}

fn option_shape(variants: &[Ty]) -> Option<String> {
    if variants.len() != 2 {
        return None;
    }
    match (&variants[0], &variants[1]) {
        (Ty::Nil, other) | (other, Ty::Nil) => Some(format!("Option<{}>", rust_ty(other))),
        _ => None,
    }
}
