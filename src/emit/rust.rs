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
use crate::dialect::HttpMethod;
use crate::expr::{ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming::snake_case;
use crate::ty::Ty;

mod controller;
mod fixture;
mod model;
mod spec;
mod view;

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
