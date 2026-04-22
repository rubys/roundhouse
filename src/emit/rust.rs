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
use crate::dialect::{Controller, HttpMethod, MethodDef, Model, Test, TestModule};
use crate::lower::{BroadcastAction, LoweredBroadcast, LoweredBroadcasts};
// Trait import for the walker — used by RsEmitter's inherent helpers
// to call `self.render_expr(...)` via trait-method dispatch.
use crate::lower::CtrlWalker as _;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::Symbol;
use crate::naming::snake_case;
use crate::ty::Ty;

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
        files.push(emit_models(app));
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
            files.push(emit_controller_axum(controller, app, &known_models));
        }
        files.push(emit_controllers_mod(&app.controllers));
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
        files.push(emit_views(app));
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
fn apply_rust_chain_modifier(prev: String, m: crate::lower::ChainModifier<'_>) -> String {
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

/// `src/controllers.rs` — declares each emitted controller submodule
/// so `src/controllers/<name>_controller.rs` files land on the module
/// tree. Separate file rather than inlining `pub mod` declarations
/// into `lib.rs` keeps the controllers directory self-contained.
fn emit_controllers_mod(controllers: &[Controller]) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    for c in controllers {
        writeln!(s, "pub mod {};", snake_case(c.name.0.as_str())).unwrap();
    }
    EmittedFile {
        path: PathBuf::from("src/controllers.rs"),
        content: s,
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

// ── Broadcaster emission ───────────────────────────────────────
//
// The broadcast declarations are parsed in `lower::broadcasts` and
// arrive here as `LoweredBroadcasts` with `Expr` leaves still
// un-rendered. This emitter converts the Expr nodes to Rust &str
// expressions (applying the lambda-param → self rewrite when
// appropriate) and writes the `impl Broadcaster for <Model>`
// block that the cable runtime's save/destroy hooks dispatch into.

/// Render a broadcast expression as a Rust &str-typed expression.
/// Bare string literals → `"str"`, interpolated strings →
/// `&format!("...{}...", self.field)`, with `<self_param>.field`
/// references rewritten to `self.field`. Falls through to a
/// `/* unsupported */` marker for shapes we haven't taught the
/// emitter yet — the generated code fails to compile loudly
/// rather than silently producing wrong HTML.
fn rust_render_broadcast_expr(expr: &Expr, self_param: Option<&Symbol>) -> String {
    let p = self_param.map(|s| s.as_str());
    match &*expr.node {
        ExprNode::Lit {
            value: Literal::Str { value },
        } => format!("{value:?}"),
        ExprNode::Lit {
            value: Literal::Int { value },
        } => format!("{value}"),
        ExprNode::Var { name, .. } => {
            if let Some(pname) = p {
                let stripped = pname.strip_prefix('_').unwrap_or(pname);
                if name.as_str() == pname || name.as_str() == stripped {
                    return "self".to_string();
                }
            }
            name.as_str().to_string()
        }
        ExprNode::Send {
            recv: Some(r),
            method,
            ..
        } => {
            let recv_s = rust_render_broadcast_expr(r, self_param);
            format!("{recv_s}.{}", method.as_str())
        }
        ExprNode::StringInterp { parts } => {
            let mut fmt = String::new();
            let mut exprs: Vec<String> = Vec::new();
            for part in parts {
                match part {
                    InterpPart::Text { value } => {
                        // Escape braces so format! treats them as
                        // literals, not placeholders.
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
                        exprs.push(rust_render_broadcast_expr(expr, self_param));
                    }
                }
            }
            format!("&format!({:?}, {})", fmt, exprs.join(", "))
        }
        _ => "/* unsupported broadcast expr */".to_string(),
    }
}

/// Emit `impl crate::cable::Broadcaster for <Model>` with the
/// lowered broadcast calls. `table_name` is the pluralized form
/// from the lowered persistence (needed for default-target naming
/// inside the cable runtime).
fn emit_rust_broadcaster_impl(
    out: &mut String,
    class: &str,
    table: &str,
    decls: &LoweredBroadcasts,
) {
    writeln!(out).unwrap();
    writeln!(out, "impl crate::cable::Broadcaster for {class} {{").unwrap();

    writeln!(out, "    fn after_save(&self) {{").unwrap();
    for b in &decls.save {
        emit_one_broadcast_call(out, class, table, b);
    }
    writeln!(out, "    }}").unwrap();

    writeln!(out, "    fn after_delete(&self) {{").unwrap();
    for b in &decls.destroy {
        emit_one_broadcast_call(out, class, table, b);
    }
    writeln!(out, "    }}").unwrap();

    writeln!(out, "}}").unwrap();
}

fn emit_one_broadcast_call(out: &mut String, class: &str, table: &str, b: &LoweredBroadcast) {
    let channel = rust_render_broadcast_expr(&b.channel, b.self_param.as_ref());
    let target = b
        .target
        .as_ref()
        .map(|t| rust_render_broadcast_expr(t, b.self_param.as_ref()))
        .unwrap_or_else(|| "\"\"".to_string());
    if let Some(assoc) = &b.on_association {
        // `<parent>.broadcast_*_to(channel)` — look the parent up by
        // foreign key and fire the broadcast on it. `if let Some`
        // swallows the missing-parent case (the common one during
        // orphaned-record cleanup and the seeding `rescue nil`).
        let var = assoc.name.as_str();
        let target_class = assoc.target_class.as_str();
        let target_table = assoc.target_table.as_str();
        let fk = assoc.foreign_key.as_str();
        writeln!(
            out,
            "        if let Some({var}) = {target_class}::find(self.{fk}) {{"
        )
        .unwrap();
        if b.action == BroadcastAction::Remove {
            writeln!(
                out,
                "            crate::cable::broadcast_remove_to({target_table:?}, {var}.id, {channel}, {target});",
            )
            .unwrap();
        } else {
            let func = action_to_fn(b.action);
            writeln!(
                out,
                "            crate::cable::{func}({target_table:?}, {var}.id, {target_class:?}, {channel}, {target});",
            )
            .unwrap();
        }
        writeln!(out, "        }}").unwrap();
        return;
    }
    if b.action == BroadcastAction::Remove {
        writeln!(
            out,
            "        crate::cable::broadcast_remove_to({table:?}, self.id, {channel}, {target});",
        )
        .unwrap();
    } else {
        let func = action_to_fn(b.action);
        writeln!(
            out,
            "        crate::cable::{func}({table:?}, self.id, {class:?}, {channel}, {target});",
        )
        .unwrap();
    }
}

fn action_to_fn(action: BroadcastAction) -> &'static str {
    match action {
        BroadcastAction::Prepend => "broadcast_prepend_to",
        BroadcastAction::Append => "broadcast_append_to",
        BroadcastAction::Replace => "broadcast_replace_to",
        BroadcastAction::Remove => "broadcast_remove_to",
    }
}

fn emit_models(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();

    // Import ValidationError from the (as-yet-unwritten) Roundhouse
    // Rust runtime if any model in this file has validations.
    // Phase 4's Rust runtime will define it; for now the `use` line
    // references an intended module that the generated code expects.
    let any_validations = app
        .models
        .iter()
        .any(|m| !crate::lower::lower_validations(m).is_empty());
    if any_validations {
        writeln!(s).unwrap();
        writeln!(s, "use crate::runtime;").unwrap();
    }

    for model in &app.models {
        writeln!(s).unwrap();
        emit_struct(&mut s, model);
        let lowered = crate::lower::lower_validations(model);
        // Always emit the impl — even models with no user-defined
        // methods or validations need the persistence interface
        // (`save`, `find`, `all`, `last`, etc.) for controllers and
        // views to consume. Skipping the impl for empty models
        // breaks generated view rendering that calls `Comment::all()`
        // etc. Abstract base classes (ApplicationRecord — no
        // attributes) stay skipped.
        if !model.attributes.fields.is_empty() {
            writeln!(s).unwrap();
            emit_model_impl(&mut s, model, &lowered, app);
        }
    }
    EmittedFile { path: PathBuf::from("src/models.rs"), content: s }
}

fn emit_struct(out: &mut String, model: &Model) {
    // Default+Clone+PartialEq cover the ergonomics tests and fixture
    // code expect: `Article::default()` for partial-init (`..Default`),
    // clone() for passing fixtures around, equality for assertions.
    // Debug is trivially free and helps test failure messages.
    writeln!(out, "#[derive(Debug, Clone, Default, PartialEq)]").unwrap();
    writeln!(out, "pub struct {} {{", model.name.0).unwrap();
    for (name, ty) in &model.attributes.fields {
        writeln!(out, "    pub {}: {},", name, rust_ty(ty)).unwrap();
    }
    // Validation error carry-over. Populated by `save()` on failure,
    // cleared on success; views render the scaffold error block when
    // this is non-empty. Skipping serde on errors is a no-op today
    // (models don't derive Serialize) but keeps the intent explicit
    // for when they do.
    writeln!(
        out,
        "    pub errors: Vec<crate::runtime::ValidationError>,",
    )
    .unwrap();
    writeln!(out, "}}").unwrap();
}

fn emit_model_impl(
    out: &mut String,
    model: &Model,
    validations: &[crate::lower::LoweredValidation],
    app: &App,
) {
    writeln!(out, "impl {} {{", model.name.0).unwrap();
    // Collect the names of attributes + methods on this class. Used by
    // emit_model_method to rewrite bare-name Sends (implicit-self calls)
    // into `self.method` when the name matches one of our members.
    let self_methods: Vec<Symbol> = model
        .attributes
        .fields
        .keys()
        .cloned()
        .chain(model.methods().map(|m| m.name.clone()))
        .collect();

    let mut first = true;
    for method in model.methods() {
        if !first {
            writeln!(out).unwrap();
        }
        first = false;
        emit_model_method(out, method, &self_methods);
    }
    if !validations.is_empty() {
        if !first {
            writeln!(out).unwrap();
        }
        emit_validate_method(out, validations);
    }
    // Persistence methods — generated for every model regardless of
    // whether it has validations, because tests may call `destroy` /
    // `count` / `find` on the class independently of validation rules.
    // Each method runs against the per-test `:memory:` SQLite
    // connection installed by `crate::db::setup_test_db`.
    if !first || !validations.is_empty() {
        writeln!(out).unwrap();
    }
    // Broadcasts: if the model declares `broadcasts_to`, save/destroy
    // end with a `crate::cable::Broadcaster::after_{save,delete}(self)`
    // call. We lower once here so emit_persistence_methods can
    // emit the hook and we can emit the Broadcaster impl after the
    // main impl closes.
    let broadcasts = crate::lower::lower_broadcasts(model);
    emit_persistence_methods(
        out,
        model,
        !validations.is_empty(),
        app,
        !broadcasts.is_empty(),
    );
    writeln!(out, "}}").unwrap();

    if !broadcasts.is_empty() {
        let lp = crate::lower::lower_persistence(model, app);
        emit_rust_broadcaster_impl(
            out,
            lp.class.0.as_str(),
            lp.table.as_str(),
            &broadcasts,
        );
    }
}

/// Render save/destroy/count/find for a model against the thread-local
/// SQLite connection. The SQL strings, column projections, belongs_to
/// checks, and dependent-destroy cascade targets come from the shared
/// `LoweredPersistence` — this function only wraps them in rusqlite
/// syntax. When `has_broadcasts` is true, save/destroy emit a trailing
/// `crate::cable::Broadcaster::after_{save,delete}(self)` call so
/// models with `broadcasts_to` push Turbo Stream frames on persist.
fn emit_persistence_methods(
    out: &mut String,
    model: &Model,
    has_validate: bool,
    app: &App,
    has_broadcasts: bool,
) {
    let lp = crate::lower::lower_persistence(model, app);
    let class = lp.class.0.as_str();

    let non_id_params: Vec<String> = lp
        .non_id_columns
        .iter()
        .map(|s| format!("self.{}", s.as_str()))
        .collect();

    // ----- save -----
    writeln!(out, "    pub fn save(&mut self) -> bool {{").unwrap();
    if has_validate {
        writeln!(out, "        let errors = self.validate();").unwrap();
        writeln!(out, "        if !errors.is_empty() {{ self.errors = errors; return false; }}").unwrap();
        writeln!(out, "        self.errors.clear();").unwrap();
    }
    // belongs_to: referenced parent must exist. Use the target's
    // `find` (which consults the same :memory: connection) so the
    // check stays in the test's transactional world.
    for check in &lp.belongs_to_checks {
        let fk = check.foreign_key.as_str();
        let target = check.target_class.0.as_str();
        writeln!(
            out,
            "        if self.{fk} == 0 || {target}::find(self.{fk}).is_none() {{",
        )
        .unwrap();
        writeln!(out, "            return false;").unwrap();
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(out, "            if self.id == 0 {{").unwrap();
    writeln!(
        out,
        "                conn.execute(\n                    {:?},\n                    rusqlite::params![{}],\n                ).expect(\"INSERT {}\");",
        lp.insert_sql,
        non_id_params.join(", "),
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "                self.id = conn.last_insert_rowid();").unwrap();
    writeln!(out, "            }} else {{").unwrap();
    writeln!(
        out,
        "                conn.execute(\n                    {:?},\n                    rusqlite::params![{}, self.id],\n                ).expect(\"UPDATE {}\");",
        lp.update_sql,
        non_id_params.join(", "),
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }});").unwrap();
    if has_broadcasts {
        writeln!(
            out,
            "        crate::cable::Broadcaster::after_save(self);"
        )
        .unwrap();
    }
    writeln!(out, "        true").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- destroy -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn destroy(&self) {{").unwrap();
    // Cascade dependent children first so their own `destroy`
    // callbacks run (matching Rails' `dependent: :destroy`), then
    // remove the parent row.
    for dc in &lp.dependent_children {
        let child_class = dc.child_class.0.as_str();
        writeln!(
            out,
            "        let dependents: Vec<{child_class}> = crate::db::with_conn(|conn| {{"
        )
        .unwrap();
        writeln!(
            out,
            "            let mut stmt = conn.prepare({:?}).expect(\"prepare child select\");",
            dc.select_by_parent_sql,
        )
        .unwrap();
        writeln!(
            out,
            "            let rows = stmt.query_map(rusqlite::params![self.id], |r| Ok({child_class} {{"
        )
        .unwrap();
        for (i, col) in dc.child_columns.iter().enumerate() {
            writeln!(out, "                {}: r.get({i})?,", col.as_str()).unwrap();
        }
        writeln!(out, "                ..Default::default()").unwrap();
        writeln!(out, "            }})).expect(\"query child rows\");").unwrap();
        writeln!(out, "            rows.filter_map(|r| r.ok()).collect()").unwrap();
        writeln!(out, "        }});").unwrap();
        writeln!(out, "        for child in &dependents {{").unwrap();
        writeln!(out, "            child.destroy();").unwrap();
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.execute({:?}, rusqlite::params![self.id])\n                .expect(\"DELETE {}\");",
        lp.delete_sql,
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "        }});").unwrap();
    if has_broadcasts {
        writeln!(
            out,
            "        crate::cable::Broadcaster::after_delete(self);"
        )
        .unwrap();
    }
    writeln!(out, "    }}").unwrap();

    // ----- count (associated function) -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn count() -> i64 {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.query_row({:?}, [], |r| r.get(0))\n                .expect(\"count {}\")",
        lp.count_sql,
        lp.table.as_str(),
    )
    .unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- find (associated function) -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn find(id: i64) -> Option<{class}> {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.query_row(\n                {:?},\n                rusqlite::params![id],",
        lp.select_by_id_sql,
    )
    .unwrap();
    writeln!(out, "                |r| Ok({class} {{").unwrap();
    for (i, field) in lp.columns.iter().enumerate() {
        writeln!(out, "                    {}: r.get({i})?,", field.as_str()).unwrap();
    }
    writeln!(out, "                    ..Default::default()").unwrap();
    writeln!(out, "                }}),\n            ).ok()").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- all (associated function) -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn all() -> Vec<{class}> {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            let mut stmt = conn.prepare({:?}).expect(\"prepare all\");",
        lp.select_all_sql,
    )
    .unwrap();
    writeln!(out, "            let rows = stmt").unwrap();
    writeln!(out, "                .query_map([], |r| Ok({class} {{").unwrap();
    for (i, field) in lp.columns.iter().enumerate() {
        writeln!(out, "                    {}: r.get({i})?,", field.as_str()).unwrap();
    }
    writeln!(out, "                    ..Default::default()").unwrap();
    writeln!(out, "                }}))").unwrap();
    writeln!(out, "                .expect(\"query all\");").unwrap();
    writeln!(out, "            rows.filter_map(|r| r.ok()).collect()").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- last (associated function) -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn last() -> Option<{class}> {{").unwrap();
    writeln!(out, "        crate::db::with_conn(|conn| {{").unwrap();
    writeln!(
        out,
        "            conn.query_row(\n                {:?},\n                [],",
        lp.select_last_sql,
    )
    .unwrap();
    writeln!(out, "                |r| Ok({class} {{").unwrap();
    for (i, field) in lp.columns.iter().enumerate() {
        writeln!(out, "                    {}: r.get({i})?,", field.as_str()).unwrap();
    }
    writeln!(out, "                    ..Default::default()").unwrap();
    writeln!(out, "                }}),\n            ).ok()").unwrap();
    writeln!(out, "        }})").unwrap();
    writeln!(out, "    }}").unwrap();

    // ----- reload (instance method) -----
    writeln!(out).unwrap();
    writeln!(out, "    pub fn reload(&mut self) {{").unwrap();
    writeln!(out, "        if let Some(fresh) = Self::find(self.id) {{").unwrap();
    writeln!(out, "            *self = fresh;").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

/// Render `fn validate(&self) -> Vec<runtime::ValidationError>` with
/// each lowered `Check` as an inline conditional. No runtime
/// validation primitives — the IR is the shared vocabulary, the
/// render is target-specific code. This is the Phase 4 showpiece:
/// the same `LoweredValidation` input feeds the TS emitter (which
/// calls Juntos's `this.validates_<kind>_of(...)` primitives) and
/// this emitter (which inlines the checks as plain Rust).
fn emit_validate_method(out: &mut String, validations: &[crate::lower::LoweredValidation]) {
    writeln!(
        out,
        "    pub fn validate(&self) -> Vec<runtime::ValidationError> {{"
    )
    .unwrap();
    writeln!(out, "        let mut errors = Vec::new();").unwrap();
    for lv in validations {
        for check in &lv.checks {
            emit_check_inline(out, lv.attribute.as_str(), check);
        }
    }
    writeln!(out, "        errors").unwrap();
    writeln!(out, "    }}").unwrap();
}

/// Render one atomic `Check` as a Rust `if` block that pushes a
/// `ValidationError` when the failure condition holds. Field access
/// uses `self.<attr>` directly — Rust's type system gives us concrete
/// access without the runtime-reflection gymnastics a generic
/// evaluator would need. Each `Check` variant maps 1:1 to a small
/// inline condition; default error message comes from the check.
fn emit_check_inline(out: &mut String, attr: &str, check: &crate::lower::Check) {
    use crate::lower::{Check, InclusionValue};
    let msg = check.default_message();
    let push = |cond: &str| -> String {
        format!(
            "        if {cond} {{\n            errors.push(runtime::ValidationError::new({attr:?}, {msg:?}));\n        }}",
        )
    };
    let block = match check {
        // Field must not be empty. The scaffold assumes a `String`
        // field; future fixtures with `Option<String>` would need
        // `self.<attr>.as_deref().unwrap_or("").is_empty()` or similar
        // — drive from a real case when one appears.
        Check::Presence => push(&format!("self.{attr}.is_empty()")),
        Check::Absence => push(&format!("!self.{attr}.is_empty()")),
        Check::MinLength { n } => push(&format!("self.{attr}.len() < {n}")),
        Check::MaxLength { n } => push(&format!("self.{attr}.len() > {n}")),
        Check::GreaterThan { threshold } => {
            push(&format!("self.{attr} <= {threshold}"))
        }
        Check::LessThan { threshold } => push(&format!("self.{attr} >= {threshold}")),
        Check::OnlyInteger => {
            // Rust's type system already distinguishes integer vs
            // float fields, so `only_integer` is semantically a
            // no-op here — the field is either i64 or f64 by Ty.
            // Emit a comment so it's clear the check was recognized
            // and handled at compile time rather than silently dropped.
            format!("        // OnlyInteger check on {attr:?} — enforced by Rust type system")
        }
        Check::Inclusion { values } => {
            let parts: Vec<String> = values.iter().map(inclusion_value_to_rust).collect();
            push(&format!(
                "![{}].contains(&self.{attr})",
                parts.join(", ")
            ))
        }
        Check::Format { pattern } => {
            // Regex dependency is a Phase-4-runtime choice; for the
            // scaffold emit a commented-out check so the generated
            // code compiles (once the runtime exists, swap in the
            // real regex call).
            format!(
                "        // TODO: Format check on {attr:?} requires runtime regex ({pattern:?})",
            )
        }
        Check::Uniqueness { .. } => {
            // Uniqueness hits the DB — it's a runtime concern, not an
            // inline check. The real runtime will provide
            // `runtime::check_uniqueness(record, attr, ...)`; until
            // then, leave a marker.
            format!(
                "        // TODO: Uniqueness check on {attr:?} requires DB access at runtime",
            )
        }
        Check::Custom { method } => {
            // User-defined method populates `errors` itself. Emit a
            // call through to it; the user is responsible for the
            // signature.
            let _ = InclusionValue::Str { value: String::new() };
            format!("        self.{method}(&mut errors);")
        }
    };
    writeln!(out, "{block}").unwrap();
}

/// Render an `InclusionValue` as a Rust literal.
fn inclusion_value_to_rust(v: &crate::lower::InclusionValue) -> String {
    use crate::lower::InclusionValue;
    match v {
        InclusionValue::Str { value } => format!("{value:?}.to_string()"),
        InclusionValue::Int { value } => format!("{value}i64"),
        InclusionValue::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { format!("{s}f64") } else { format!("{s}.0f64") }
        }
        InclusionValue::Bool { value } => value.to_string(),
    }
}

fn emit_model_method(out: &mut String, m: &MethodDef, self_methods: &[Symbol]) {
    let ret_ty = m.body.ty.clone().unwrap_or(Ty::Nil);
    let receiver = match m.receiver {
        crate::dialect::MethodReceiver::Instance => "&self",
        crate::dialect::MethodReceiver::Class => "",
    };
    writeln!(
        out,
        "    pub fn {}({}) -> {} {{",
        m.name,
        receiver,
        rust_ty(&ret_ty),
    )
    .unwrap();
    let ctx = EmitCtx {
        self_methods,
        ..EmitCtx::default()
    };
    for line in emit_expr(&m.body, ctx).lines() {
        writeln!(out, "        {}", line).unwrap();
    }
    writeln!(out, "    }}").unwrap();
}

// Controllers ----------------------------------------------------------
//
// Phase 4c: every action and private helper is emitted as a free-standing
// `fn` inside the controller's `impl` block. Bodies go through the same
// `emit_body` / `emit_expr` machinery as model methods, with a few
// controller-specific Send rewrites turned on via `EmitCtx::in_controller`:
//
//   * bare `params`          → `crate::http::params()`
//   * `params.expect(...)`   → `todo!("params.expect")` (divergent, so
//                              its `!` type unifies with whatever the
//                              call site expects, e.g. `i64` in
//                              `Article::find(params.expect(:id))`)
//   * `respond_to { ... }`   → `crate::http::respond_to(|__fr| { ... })`
//   * `format.html { body }` → `__fr.html(|| { body })`
//   * `format.json { body }` → emitted as a comment placeholder (the
//                              JSON branch is deferred per scope)
//   * `redirect_to`, `render`, `head` (bare) → `crate::http::*`
//   * `x.destroy!()`         → `x.destroy()` (Rust forbids `!` in idents)
//   * bare Send to a name in `self_methods` → `Self::name(...)`
//     (so `article_params` inside `create` compiles as `Self::article_params()`)
//
// Action bodies always return `crate::http::Response`. The emitter
// discards each action body's natural tail (Rails' convention: ivars
// feed the view) and appends `crate::http::Response::default()`.

/// Emit `src/views.rs` — view renderers derived from the ingested
/// `.html.erb` templates.
///
/// ERB compilation to Ruby (`_buf = _buf + chunk`) happens at
/// ingestion (`src/erb.rs`); each view lands as an `Expr` whose body
/// is a `Seq` of `_buf` assignments. This emitter walks that shape
/// and renders per-statement into Rust string-building. Unknown
/// Rails helpers fall through as function calls against
/// `crate::view_helpers::*`, where a hand-written runtime supplies
/// HTML-producing stubs.
///
/// What's handled here:
///   - `_buf = ""` prologue + bare `_buf` epilogue → dropped
///     (emitter adds its own `let mut _buf = String::new();` +
///     tail `_buf`).
///   - `_buf = _buf + "text"` → `_buf.push_str("text");`
///   - `_buf = _buf + (expr).to_s` → `_buf.push_str(&expr.to_string());`
///   - `if/else` with buf-appending branches → passthrough.
///   - `<coll>.each do |x| ... end` with buf-appending body → `for
///     x in &coll { ... }`.
///   - `render @coll` / `render @x.assoc` → expand to a for loop
///     calling the matching `render_<singular>` partial.
///   - Helper calls with blocks (form_with, content_for) → emit the
///     block body into a fresh scratch buffer so the helper's
///     wrapping return value composes correctly.
///
/// View fn signatures follow the resource layout:
///   - `<resource>_index(records: &[Model]) -> String`
///   - `<resource>_show(record: &Model) -> String`
///   - `<resource>_new(record: &Model) -> String` (form scaffold)
///   - `<resource>_edit(record: &Model) -> String`
///   - `render_<singular>(record: &Model) -> String` (partial)
fn emit_views(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "#![allow(unused_imports, unused_variables, unused_mut)]").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use crate::models::*;").unwrap();
    writeln!(s, "use crate::route_helpers;").unwrap();
    writeln!(s, "use crate::view_helpers::{{self, FormBuilder, RenderCtx}};").unwrap();
    writeln!(s).unwrap();

    // has_many lookup table — shared across view emitters via
    // `src/lower/associations`. Used by view emit to lower
    // `article.comments` shapes into inline filter queries.
    let has_manys = crate::lower::build_has_many_table(app);

    let view_ctx = ViewEmitCtx {
        known_models: app.models.iter().map(|m| m.name.0.clone()).collect(),
        locals: Vec::new(),
        local_attrs: Vec::new(),
        arg_name: String::new(),
        resource_dir: String::new(),
        form_records: Vec::new(),
        stylesheets: app.stylesheets.clone(),
        has_manys,
    };

    // Build a class-to-attrs map used when a view fn's arg resolves
    // to a model struct, so the simple-expr check can allow
    // `article.title` while rejecting `article.errors`.
    let attrs_by_class: std::collections::BTreeMap<String, Vec<Symbol>> = app
        .models
        .iter()
        .map(|m| {
            (
                m.name.0.as_str().to_string(),
                m.attributes.fields.keys().cloned().collect(),
            )
        })
        .collect();
    let attrs_by_class = &attrs_by_class;

    // Emit one function per view. Partials (`_foo.html.erb`) render
    // as `render_<name>` taking the partial's record. Top-level
    // views take the resource's fixture.
    for view in &app.views {
        emit_view_fn(&mut s, view, &view_ctx, attrs_by_class);
        writeln!(s).unwrap();
    }

    // Controllers reference standard CRUD views by name (articles_
    // show, articles_new, etc.). When a template doesn't exist in
    // the fixture (tiny-blog has only an index template), emit a
    // stub fn returning an empty string so the controller call
    // sites still compile.
    emit_missing_view_stubs(&mut s, app, &view_ctx);

    EmittedFile {
        path: PathBuf::from("src/views.rs"),
        content: s,
    }
}

#[derive(Clone)]
struct ViewEmitCtx {
    known_models: Vec<Symbol>,
    /// Names bound in the current view fn's scope — the fn's arg
    /// name plus any `|param|`s introduced by block literals (each,
    /// form_with yielded FormBuilder, etc.). Bare Sends with no recv
    /// and no args that match a local name emit as the bare name,
    /// not as a `view_helpers::name()` call.
    locals: Vec<String>,
    /// Per-local attribute list, keyed by local name. Populated when
    /// the local binds a known-model struct (view fn arg, each-loop
    /// var). `is_simple_view_expr` consults this to allow
    /// `article.title` emits while rejecting `article.errors` (not
    /// in the attribute set → no method exists on the Rust struct).
    local_attrs: Vec<(String, Vec<Symbol>)>,
    /// The first binding in the current view fn's scope — the
    /// view's primary arg name (`article` for `articles/new`,
    /// `comment` for `comments/_comment`). form_with defaults the
    /// model kwarg to this when the Ruby source doesn't pass one,
    /// matching Rails' partial convention.
    arg_name: String,
    /// Plural resource directory of the current view (`articles`
    /// for `articles/new`, `comments` for `comments/_form`). Used
    /// to derive the path-helper names for the form action URL.
    resource_dir: String,
    /// Active FormBuilder bindings: `(builder_local_name,
    /// record_expr)`. Populated by `emit_captured_block_helper` on
    /// entry to a `form_with` block and consulted by the form-
    /// method emit so `form.text_field :title` can emit
    /// `form.text_field("title", &article.title, ...)`.
    form_records: Vec<(String, String)>,
    /// Discovered stylesheet names (stems). Feeds the
    /// `stylesheet_link_tag :app` expansion in the layout.
    stylesheets: Vec<String>,
    /// Has-many associations flattened via
    /// `crate::lower::build_has_many_table`. Lets view emit lower
    /// `owner.assoc` reads into inline filter queries without
    /// needing `&App` threaded through ViewEmitCtx.
    has_manys: Vec<crate::lower::HasManyRow>,
}

impl ViewEmitCtx {
    fn with_locals(&self, more: impl IntoIterator<Item = String>) -> Self {
        let mut next = self.clone();
        for n in more {
            if !next.locals.iter().any(|x| x == &n) {
                next.locals.push(n);
            }
        }
        next
    }

    fn with_local_attrs(&self, name: String, attrs: Vec<Symbol>) -> Self {
        let mut next = self.clone();
        if !next.locals.iter().any(|x| x == &name) {
            next.locals.push(name.clone());
        }
        next.local_attrs.retain(|(n, _)| n != &name);
        next.local_attrs.push((name, attrs));
        next
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.iter().any(|x| x == name)
    }

    fn local_has_attr(&self, local: &str, attr: &str) -> bool {
        self.local_attrs
            .iter()
            .find(|(n, _)| n == local)
            .map(|(_, attrs)| attrs.iter().any(|a| a.as_str() == attr))
            .unwrap_or(false)
    }

    /// Resolve `<local>.<assoc>` where `local` is a known local
    /// bound to a model and `assoc` is the association name.
    /// Returns (target_class, foreign_key) when matched. The owner
    /// class is inferred from the local name via the scaffold
    /// naming convention: `article` → `Article` → the has_many
    /// entries on `Article`.
    fn resolve_has_many_on_local(&self, local: &str, assoc: &str) -> Option<(String, String)> {
        if !self.is_local(local) {
            return None;
        }
        crate::lower::resolve_has_many_on_local(&self.has_manys, local, assoc)
    }
}

/// Emit one view as a Rust fn. Derives the fn name + signature from
/// the view's path (`articles/index` → `articles_index(records: &[
/// Article])`, `articles/_article` → `render_article(article: &
/// Article)`).
fn emit_view_fn(
    out: &mut String,
    view: &crate::dialect::View,
    ctx: &ViewEmitCtx,
    attrs_by_class: &std::collections::BTreeMap<String, Vec<Symbol>>,
) {
    let name = view.name.as_str();
    let (fn_name, sig, arg_name) = view_fn_signature(name, ctx);
    writeln!(out, "pub fn {fn_name}({sig}) -> String {{").unwrap();
    writeln!(out, "    let mut _buf = String::new();").unwrap();
    writeln!(out, "    let ctx = RenderCtx::default();").unwrap();

    // The fn's argument is a view-scope local. Add to ctx so bare
    // uses in the template (`<%= article.title %>`) don't route
    // through view_helpers. Look up the arg's model class (if any)
    // to seed per-local attribute knowledge for the simple-expr
    // check.
    let model_class = arg_model_class(name, ctx);
    // Derive the resource directory from the view path (the first
    // path segment is the plural resource name: `articles/new` →
    // `articles`, `comments/_form` → `comments`). Used by form_with
    // to compute the path-helper for the action URL.
    let resource_dir = name.split('/').next().unwrap_or("").to_string();
    let mut scoped = if let Some(class) = model_class {
        let attrs = attrs_by_class.get(&class).cloned().unwrap_or_default();
        ctx.with_local_attrs(arg_name.clone(), attrs)
    } else {
        ctx.with_locals([arg_name.clone()])
    };
    scoped.arg_name = arg_name.clone();
    scoped.resource_dir = resource_dir;
    // `_buf` is an emitted local; it's also in scope.
    scoped = scoped.with_locals(["_buf".to_string(), "ctx".to_string()]);

    // Apply the shared erubi-trim lowering — same as TS. The
    // body arrives with `<% %>` indent/newline trim already
    // applied and the trailing whitespace-only append dropped,
    // so emit walks it straight.
    let trimmed_body = crate::lower::erb_trim::trim_view(&view.body);
    let stmts: Vec<&Expr> = match &*trimmed_body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![&trimmed_body],
    };
    for stmt in &stmts {
        for line in emit_view_stmt_rust(stmt, &scoped, "_buf") {
            writeln!(out, "    {line}").unwrap();
        }
    }
    writeln!(out, "    _buf").unwrap();
    writeln!(out, "}}").unwrap();
}

/// Emit empty-body view fn stubs for the standard Rails CRUD views
/// (index / show / new / edit) when the fixture's templates are
/// missing. Keeps the controller emit's references satisfied.
fn emit_missing_view_stubs(out: &mut String, app: &App, ctx: &ViewEmitCtx) {
    use std::collections::BTreeSet;
    let present: BTreeSet<String> =
        app.views.iter().map(|v| v.name.as_str().to_string()).collect();
    for model in &app.models {
        if model.attributes.fields.is_empty() {
            continue;
        }
        let class = model.name.0.as_str();
        let plural = crate::naming::pluralize_snake(class);
        for action in ["index", "show", "new", "edit"] {
            let name = format!("{plural}/{action}");
            if present.contains(&name) {
                continue;
            }
            let (fn_name, sig, _arg) = view_fn_signature(&name, ctx);
            writeln!(out, "pub fn {fn_name}({sig}) -> String {{").unwrap();
            writeln!(out, "    String::new()").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
        }
    }
}

/// Look up the model class for the view fn's argument, if any.
/// `articles/show` → `Some("Article")`; `articles/index` →
/// `Some("Article")` too (the arg is `&[Article]` but per-element
/// attr access is the same). Templates without a known model arg
/// (unusual) return None.
fn arg_model_class(view_name: &str, ctx: &ViewEmitCtx) -> Option<String> {
    let (dir, _) = view_name.rsplit_once('/').unwrap_or(("", view_name));
    let class = crate::naming::singularize_camelize(dir);
    if ctx.known_models.iter().any(|m| m.as_str() == class) {
        Some(class)
    } else {
        None
    }
}

/// Derive (fn_name, arg_list, arg_name) from a view's relative
/// path. The third element is the name of the parameter so the
/// emitter can add it to the view scope's locals.
fn view_fn_signature(name: &str, ctx: &ViewEmitCtx) -> (String, String, String) {
    let (dir, base) = name.rsplit_once('/').unwrap_or(("", name));
    let resource = dir;
    let is_partial = base.starts_with('_');
    let stem = base.trim_start_matches('_');
    let model_class = crate::naming::singularize_camelize(resource);
    let model_exists = ctx.known_models.iter().any(|m| m.as_str() == model_class);
    let singular = crate::naming::singularize(resource);

    if is_partial {
        // Partial fn name: `render_<stem>`. Arg name: the model
        // singular when it's a known model (so the template's
        // `article.title` maps to our arg), otherwise the partial's
        // stem (used for non-model partials like `_form`).
        let fn_name = format!("render_{stem}");
        let (arg_name, arg_ty) = if model_exists {
            (singular.clone(), format!("{singular}: &{model_class}"))
        } else {
            (stem.to_string(), format!("{stem}: &crate::models::{model_class}"))
        };
        (fn_name, arg_ty, arg_name)
    } else {
        let fn_name = format!("{resource}_{stem}");
        let (arg_name, sig) = match stem {
            "index" => {
                if model_exists {
                    (resource.to_string(), format!("{resource}: &[{model_class}]"))
                } else {
                    (resource.to_string(), format!("{resource}: &[()]"))
                }
            }
            _ => {
                if model_exists {
                    (singular.clone(), format!("{singular}: &{model_class}"))
                } else {
                    (singular.clone(), format!("{singular}: &()"))
                }
            }
        };
        (fn_name, sig, arg_name)
    }
}

/// Render one view-body statement. Returns the lines to emit (one
/// statement is often multiple Rust lines for block forms).
/// `buf_name` is the local buffer variable to append to — usually
/// `_buf`, but switches to `__inner` inside a captured-block helper.
fn emit_view_stmt_rust(stmt: &Expr, ctx: &ViewEmitCtx, buf_name: &str) -> Vec<String> {
    match &*stmt.node {
        // Prologue `_buf = ""` → drop (we emit our own).
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == buf_name =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == buf_name {
                            return emit_view_append(&args[0], ctx, buf_name);
                        }
                    }
                }
            }
            // Other `_buf = X` shape — pass through, but this
            // shouldn't happen with the current ERB compiler.
            vec![format!("/* unexpected _buf shape */ {};", emit_view_expr(stmt, ctx))]
        }
        // Epilogue: bare `_buf` read → drop (we emit `_buf` as the
        // tail return).
        ExprNode::Var { name, .. } if name.as_str() == buf_name => Vec::new(),
        // `<% if cond %>...<% end %>` → passthrough `if/else`.
        // Complex conds (`article.errors.any?`, `notice.present?`,
        // etc.) degrade to `false` so the then-branch never fires
        // but the else branch still compiles.
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = if is_simple_view_expr(cond, ctx) {
                emit_view_expr_raw(cond, ctx)
            } else {
                "false /* TODO ERB cond */".to_string()
            };
            let mut out = vec![format!("if {cond_s} {{")];
            for line in emit_view_branch_rust(then_branch, ctx, buf_name) {
                out.push(format!("    {line}"));
            }
            let has_else = !matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if has_else {
                out.push("} else {".to_string());
                for line in emit_view_branch_rust(else_branch, ctx, buf_name) {
                    out.push(format!("    {line}"));
                }
            }
            out.push("}".to_string());
            out
        }
        // `<% content_for :slot, "value" %>` — statement-form
        // setter. Classifier recognizes the shape; render the
        // side-effect call without a `_buf` append.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if matches!(
                crate::lower::classify_view_helper(method.as_str(), args),
                Some(crate::lower::ViewHelperKind::ContentForSetter { .. })
            ) =>
        {
            if let Some(crate::lower::ViewHelperKind::ContentForSetter { slot, body }) =
                crate::lower::classify_view_helper(method.as_str(), args)
            {
                if is_simple_view_expr(body, ctx) {
                    let body_s = emit_view_expr_raw(body, ctx);
                    return vec![format!(
                        "view_helpers::content_for_set({slot:?}, &{body_s});"
                    )];
                }
            }
            vec!["/* TODO ERB: content_for with complex body */".to_string()]
        }
        // `<% @coll.each do |x| %>...<% end %>` → `for x in &coll`.
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if method.as_str() == "each" && args.is_empty() =>
        {
            emit_view_each_rust(recv, block, ctx, buf_name)
        }
        // Fall through — any unrecognized expression statement.
        _ => vec![format!("{};", emit_view_expr(stmt, ctx))],
    }
}

fn emit_view_branch_rust(expr: &Expr, ctx: &ViewEmitCtx, buf_name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let stmts: Vec<&Expr> = match &*expr.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![expr],
    };
    for stmt in &stmts {
        out.extend(emit_view_stmt_rust(stmt, ctx, buf_name));
    }
    out
}

fn emit_view_each_rust(
    coll: &Expr,
    block: &Expr,
    ctx: &ViewEmitCtx,
    buf_name: &str,
) -> Vec<String> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return vec![format!("/* unexpected each block */")];
    };
    // Complex collection expressions (e.g. `article.errors.each`)
    // degrade to a skipped loop body — the collection would be a
    // placeholder String rather than an iterable, so the loop
    // would fail to compile.
    if !is_simple_view_expr(coll, ctx) {
        return vec!["/* TODO ERB: each over complex collection */".to_string()];
    }
    let coll_s = emit_view_expr_raw(coll, ctx);
    let var = params.first().map(|p| p.as_str().to_string()).unwrap_or_else(|| "item".into());
    // `.iter()` works uniformly on `&[T]` (view fn arg) and `Vec<T>`
    // (inline has_many filter result / `article.errors`), yielding
    // `&T`. Prefixing with `&` would fail on `&[T]` because `&&[T]`
    // isn't an iterator.
    let mut out = vec![format!("for {var} in {coll_s}.iter() {{")];
    let body_ctx = ctx.with_locals([var.clone()]);
    for line in emit_view_branch_rust(body, &body_ctx, buf_name) {
        out.push(format!("    {line}"));
    }
    out.push("}".to_string());
    out
}

/// Emit the RHS of `_buf = _buf + X` — either a text chunk or a
/// `<%= expr %>` interpolation. Text chunks are always faithful
/// (the literal HTML). Interpolations are faithful only when the
/// expression is simple (model attribute access, bare locals,
/// `render @coll` expansion); complex expressions (FormBuilder
/// chains, `.errors` lookups, helpers with models-as-args) degrade
/// to an empty-string placeholder so the rest of the view still
/// compiles.
///
/// The degradation is deliberate: faithfully rendering real-blog's
/// full ERB surface needs substantial Rails-runtime reimplementation
/// (FormBuilder, ValidationErrors collections, has_many accessors,
/// dom_id conventions, pluralize/truncate inflectors, …). That work
/// is scoped to a later phase. For Phase 4d's test bar, the tests
/// check a handful of literal tags (`<h1>`, `<h2>`, `<form>`,
/// `id="articles"`, `class="p-4"`) that all live in text chunks —
/// so dropping complex interpolations keeps the tests green.
fn emit_view_append(arg: &Expr, ctx: &ViewEmitCtx, buf_name: &str) -> Vec<String> {
    // Text chunk: arg is a string literal.
    if let ExprNode::Lit { value: Literal::Str { value: s } } = &*arg.node {
        return vec![format!("{buf_name}.push_str({s:?});")];
    }
    // Peel the ERB compiler's `.to_s` wrapper.
    let inner = unwrap_to_s_rust(arg);

    // `render @coll` / `render "name", locals_hash` — expand.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if method.as_str() == "render" {
            if args.len() == 1 {
                let loop_expr = emit_view_render_call(&args[0], ctx);
                return vec![format!("{buf_name}.push_str(&{loop_expr});")];
            }
            // `render "partial", key: value, ...` — two-arg form.
            if args.len() == 2 {
                if let (
                    ExprNode::Lit { value: Literal::Str { value: partial } },
                    ExprNode::Hash { entries, .. },
                ) = (&*args[0].node, &*args[1].node)
                {
                    // Pick the first local-named kwarg that
                    // singularizes to the same name as the partial
                    // (matches Rails' scaffold convention, e.g.
                    // `render "form", article: @article`).
                    let partial_fn = format!("render_{partial}");
                    for (k, v) in entries {
                        if let ExprNode::Lit { value: Literal::Sym { value: kname } } = &*k.node {
                            let arg_expr = emit_view_expr(v, ctx);
                            // Strip any `&` prefix — the partial fn
                            // takes `&T`, and emit_view_expr returns
                            // the local name for an Ivar/Var which
                            // is already `&T` in scope.
                            let _ = kname;
                            return vec![format!(
                                "{buf_name}.push_str(&{partial_fn}({arg_expr}));"
                            )];
                        }
                    }
                }
            }
        }
    }

    // Capturing helpers (form_with, content_for) — the block body
    // accumulates into a scratch buffer so the helper can wrap it.
    // Handled before the simple-check because form_with never
    // passes `is_simple_view_expr` (its block body is complex).
    if let ExprNode::Send {
        recv: None,
        method,
        args,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if is_capturing_helper(method.as_str()) {
            return emit_captured_block_helper(
                method.as_str(),
                args,
                block,
                ctx,
                buf_name,
            );
        }
    }

    // `<%= yield %>` / `<%= yield :slot %>` — ExprNode::Yield.
    if let ExprNode::Yield { args } = &*inner.node {
        if args.is_empty() {
            return vec![format!("{buf_name}.push_str(&view_helpers::get_yield());")];
        }
        if args.len() == 1 {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
                return vec![format!(
                    "{buf_name}.push_str(&view_helpers::get_slot({:?}));",
                    value.as_str(),
                )];
            }
        }
    }

    // Shared view-helper classifier — single recognition step for
    // every Rails helper (csrf_meta_tags, link_to, dom_id,
    // pluralize, truncate, etc).
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if let Some(kind) = crate::lower::classify_view_helper(method.as_str(), args) {
            if let Some(line) = emit_rust_view_helper(&kind, ctx, buf_name) {
                return line;
            }
        }
    }

    // `content_for(:slot) || "fallback"` — BoolOp (Or); not a
    // Send so the classifier doesn't see the outer.
    if let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } = &*inner.node {
        if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*left.node {
            if let Some(crate::lower::ViewHelperKind::ContentForGetter { slot }) =
                crate::lower::classify_view_helper(method.as_str(), args)
            {
                if is_simple_view_expr(right, ctx) {
                    let fallback = emit_view_expr_raw(right, ctx);
                    return vec![format!(
                        "{{ let __cf = view_helpers::content_for_get({slot:?}); let __fb = {fallback}; {buf_name}.push_str(if __cf.is_empty() {{ &__fb }} else {{ __cf.as_str() }}); }}"
                    )];
                }
            }
        }
    }

    // FormBuilder method call: `form.label :title, class: "..."`.
    // Classifier gives us the method kind; rust maps to snake_case
    // method names directly.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*inner.node {
        if let ExprNode::Var { name: recv_name, .. } = &*r.node {
            if ctx.is_local(recv_name.as_str()) {
                if let Some(fb) = crate::lower::classify_form_builder_method(method.as_str()) {
                    let rs_method = match fb {
                        crate::lower::FormBuilderMethod::Label => "label",
                        crate::lower::FormBuilderMethod::TextField => "text_field",
                        crate::lower::FormBuilderMethod::TextArea => "textarea",
                        crate::lower::FormBuilderMethod::Submit => "submit",
                    };
                    if let Some(call) = emit_rust_form_builder_call(
                        recv_name.as_str(),
                        rs_method,
                        args,
                        ctx,
                    ) {
                        return vec![format!("{buf_name}.push_str(&{call});")];
                    }
                }
            }
        }
    }

    // Simple interpolation: `@record.attr` or bare local → emit as
    // `.to_string()` append. Anything else degrades.
    if is_simple_view_expr(inner, ctx) {
        return vec![format!(
            "{buf_name}.push_str(&{}.to_string());",
            emit_view_expr(inner, ctx),
        )];
    }

    // Complex expression (form_with block, link_to with model,
    // pluralize with inflection, etc.) — degrade to empty string
    // with a TODO comment so the emitted source documents the gap.
    vec![format!(
        "/* TODO ERB: complex expression — see fixture view source */ {buf_name}.push_str(\"\");",
    )]
}

/// True when a view-body expression is safe to render as-is with
/// `emit_view_expr`. Criteria (deliberately narrow to make the
/// guarantee easy to honor):
///   - Literal value
///   - Bare local variable (view fn arg, loop var)
///   - Method-chain read on a local (`article.title`,
///     `article.body`) with sanitizable method names and zero args
///   - String interpolation whose parts are themselves simple
fn is_simple_view_expr(expr: &Expr, ctx: &ViewEmitCtx) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => ctx.is_local(name.as_str()),
        // Partial-scope locals — Prism parses a bare `article`
        // inside `_article.html.erb` as `Send { recv: None,
        // method: "article", args: [] }` because the compiled ERB
        // wrapper doesn't formally introduce the arg as a Ruby
        // local before use. Treat as a simple local-read when the
        // method matches a ctx local.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && ctx.is_local(method.as_str()) =>
        {
            true
        }
        ExprNode::Send { recv: Some(r), method, args, block, .. } => {
            if !args.is_empty() || block.is_some() {
                return false;
            }
            let clean = sanitize_method_name(method.as_str());
            if clean.is_empty() {
                return false;
            }
            let recv_local = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.as_str()),
                ExprNode::Send {
                    recv: None,
                    method: m,
                    args: ra,
                    block: None,
                    ..
                } if ra.is_empty() && ctx.is_local(m.as_str()) => Some(m.as_str()),
                _ => None,
            };
            if let Some(local_name) = recv_local {
                if ctx.local_has_attr(local_name, &clean) {
                    return true;
                }
                if ctx.is_local(local_name)
                    && matches!(method.as_str(), "any?" | "none?" | "present?" | "empty?")
                {
                    return true;
                }
                // has_many association: `article.comments` where
                // Article.has_many :comments. Lowers to an inline
                // filter query.
                if ctx.resolve_has_many_on_local(local_name, method.as_str()).is_some() {
                    return true;
                }
            }
            // Chained simple Sends — `article.comments.size` where
            // the inner `article.comments` is has_many. Recursively
            // accept when the recv is simple.
            if is_simple_view_expr(r, ctx) {
                return true;
            }
            false
        }
        ExprNode::StringInterp { parts } => parts.iter().all(|p| match p {
            crate::expr::InterpPart::Text { .. } => true,
            crate::expr::InterpPart::Expr { expr } => is_simple_view_expr(expr, ctx),
        }),
        _ => false,
    }
}

fn unwrap_to_s_rust(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

/// Is this a helper that takes a block and wraps the block's output?
/// `form_with` wraps in a `<form>` tag; `content_for` stashes the
/// block into a named slot for later layout render. Both need the
/// block body to accumulate into a scratch buffer rather than outer
/// `_buf`.
fn is_capturing_helper(method: &str) -> bool {
    matches!(method, "form_with" | "content_for")
}

/// Emit a captured-block helper call. Block body renders into a
/// scratch `__inner` buffer; the helper receives it and returns a
/// wrapped String that gets appended to outer `_buf`.
fn emit_captured_block_helper(
    method: &str,
    args: &[Expr],
    block: &Expr,
    ctx: &ViewEmitCtx,
    outer_buf: &str,
) -> Vec<String> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return vec![format!("/* unexpected {method} block */")];
    };

    let html_class = extract_kwarg(args, "class")
        .filter(|e| is_simple_view_expr(e, ctx))
        .map(|e| emit_view_expr_raw(e, ctx))
        .unwrap_or_else(|| "String::new()".to_string());

    // Resolve the record reference the form is bound to. Three
    // shapes:
    //  1. `model: @record` — simple; use the record directly.
    //  2. `model: [@parent, Child.new]` — nested; the last element
    //     is the form's record; preceding elements scope the action
    //     URL through a nested path helper.
    //  3. No `model:` kwarg: fall back to the view's primary arg
    //     (Rails partial convention).
    let model_expr = extract_kwarg(args, "model");
    let model_nested: Option<Vec<Expr>> = model_expr.and_then(|e| match &*e.node {
        ExprNode::Array { elements, .. } if elements.len() >= 2 => Some(elements.clone()),
        _ => None,
    });
    let record_ref: Option<String> = if let Some(elems) = &model_nested {
        Some(emit_rust_nested_form_record(&elems[elems.len() - 1]))
    } else {
        model_expr
            .filter(|e| is_simple_view_expr(e, ctx))
            .map(|e| emit_view_expr_raw(e, ctx))
            .or_else(|| {
                if !ctx.arg_name.is_empty() && ctx.is_local(&ctx.arg_name) {
                    Some(ctx.arg_name.clone())
                } else {
                    None
                }
            })
    };

    // Singular prefix for field names (`article[title]`) — derived
    // from the resource directory when present, or arg_name fallback.
    // For nested array-form models, the prefix is the CHILD class
    // (last element).
    let prefix = if let Some(elems) = &model_nested {
        rust_nested_form_child_prefix(&elems[elems.len() - 1])
            .unwrap_or_else(|| ctx.arg_name.clone())
    } else if !ctx.resource_dir.is_empty() {
        crate::naming::singularize(&ctx.resource_dir)
    } else {
        ctx.arg_name.clone()
    };

    let mut out = Vec::new();
    // Use a block-expression so the inner `_buf` shadow is confined
    // to the block's scope. The block evaluates to the wrapped-form
    // String, which we then push onto the outer buf after the block
    // closes — so the outer `_buf` identifier doesn't get captured
    // by the inner shadow.
    out.push(format!("{outer_buf}.push_str(&{{"));
    // Shadow `_buf` inside the block so the compiled ERB's
    // `_buf = _buf + X` statements accumulate into an inner scratch
    // buffer instead of the outer view buf. The block's final
    // expression returns the wrapped inner buf.
    out.push("    let mut _buf = String::new();".to_string());

    if let Some(p) = params.first() {
        let pname = p.as_str();
        match method {
            "form_with" => {
                let is_persisted_expr = record_ref
                    .as_deref()
                    .map(|r| format!("{r}.id != 0"))
                    .unwrap_or_else(|| "false".to_string());
                out.push(format!(
                    "    let {pname} = FormBuilder::new(\"{prefix}\", &{html_class}, {is_persisted_expr});",
                ));
                // Prepend the scaffold error block so validation
                // failures show visibly in the form. Matches the TS
                // emitter's approach and bypasses the generic
                // `<% if record.errors.any? %>` lowering (which
                // still stubs to `if false`). Only fires when the
                // record has accumulated errors — `validate()` on a
                // fresh record returns errors unconditionally, so we
                // gate on `!errors.is_empty()` here so new-record
                // forms don't display errors pre-submit; the actual
                // post-failed-save flow passes through the same
                // fresh record but with populated errors once the
                // model runtime grows an error-state field.
                if let Some(record) = &record_ref {
                    out.push(format!(
                        "    _buf.push_str(&view_helpers::error_messages_for(&{record}.errors, \"{prefix}\"));",
                    ));
                }
            }
            _ => {
                out.push(format!("    let {pname} = ();"));
            }
        }
    }

    // Register the yielded form builder as a local so form.<method>
    // calls in the block body can be recognized, plus map it to the
    // bound record so text_field / textarea can emit the field-value
    // access at the call site.
    let inner_ctx = if let Some(p) = params.first() {
        let mut c = ctx.with_locals([p.as_str().to_string()]);
        if let Some(record) = &record_ref {
            c.form_records
                .push((p.as_str().to_string(), record.clone()));
        }
        c
    } else {
        ctx.clone()
    };
    // Walk block body with buf_name = "_buf" (the inner, shadowing
    // binding). The `_buf = _buf + X` assignments inside the block
    // match the usual assign pattern and dispatch to emit_view_append.
    for line in emit_view_branch_rust(body, &inner_ctx, "_buf") {
        out.push(format!("    {line}"));
    }

    match method {
        "form_with" => {
            // Resource path: new records POST to the collection URL
            // (`articles_path()`), persisted records PATCH to the
            // member URL (`article_path(id)`). The runtime form_wrap
            // inserts the `_method=patch` hidden input when
            // persisted.
            let plural = if !ctx.resource_dir.is_empty() {
                ctx.resource_dir.clone()
            } else {
                crate::naming::pluralize_snake(&prefix)
            };
            let (resource_path_expr, is_persisted_expr) = match (&record_ref, &model_nested) {
                (Some(record), Some(elems)) => (
                    rust_nested_form_path_expr(elems, ctx, record, &prefix),
                    format!("{record}.id != 0"),
                ),
                (Some(record), None) => (
                    format!(
                        "if {record}.id != 0 {{ route_helpers::{prefix}_path({record}.id) }} else {{ route_helpers::{plural}_path() }}",
                    ),
                    format!("{record}.id != 0"),
                ),
                (None, _) => (
                    format!("route_helpers::{plural}_path()"),
                    "false".to_string(),
                ),
            };
            out.push(format!(
                "    let __action = {resource_path_expr};",
            ));
            // Tail expression — the block evaluates to the wrapped
            // form HTML, which the outer push_str consumes.
            out.push(format!(
                "    view_helpers::form_wrap(&__action, {is_persisted_expr}, &{html_class}, &_buf)",
            ));
        }
        "content_for" => {
            // Stashed into layout-slot storage (not wired yet) —
            // the block evaluates to an empty string so the outer
            // push_str appends nothing.
            out.push("    let _ = _buf; String::new()".to_string());
        }
        _ => {
            out.push("    String::new()".to_string());
        }
    }

    out.push("});".to_string());
    out
}

/// Emit the record expression for the CHILD element of a nested
/// form_with's `model:` array (`[parent, Child.new]` or similar).
/// Dispatches on the shared `NestedFormChild` classifier.
fn emit_rust_nested_form_record(el: &Expr) -> String {
    match crate::lower::classify_nested_form_child(el) {
        Some(crate::lower::NestedFormChild::ClassNew { class }) => format!("{class}::default()"),
        Some(crate::lower::NestedFormChild::Local { name }) => name.to_string(),
        None => "Default::default()".to_string(),
    }
}

/// Extract the singular prefix for a nested form_with's child
/// element — `Comment.new` → `"comment"`, bare `comment` local →
/// `"comment"`.
fn rust_nested_form_child_prefix(el: &Expr) -> Option<String> {
    crate::lower::classify_nested_form_child(el).map(|k| k.prefix())
}

/// Build the form action URL for a nested-resource form_with.
/// `elems` is the `[parent, child]` array; `record_ref` is the
/// Rust expression for the child. When the child has an `id != 0`
/// (persisted), emit the member path; otherwise the collection
/// path.
fn rust_nested_form_path_expr(
    elems: &[Expr],
    ctx: &ViewEmitCtx,
    record_ref: &str,
    child_prefix: &str,
) -> String {
    let is_local = |n: &str| ctx.is_local(n);
    let mut parent_ids: Vec<String> = Vec::new();
    let mut parent_singulars: Vec<String> = Vec::new();
    for parent in &elems[..elems.len() - 1] {
        let Some(kind) = crate::lower::classify_nested_url_element(parent, &is_local) else {
            return "String::new()".to_string();
        };
        let (singular, id_expr) = rust_nested_element_parts(&kind);
        parent_singulars.push(singular);
        parent_ids.push(id_expr);
    }
    let member_name = format!(
        "{}_{}_path",
        parent_singulars.join("_"),
        child_prefix,
    );
    let child_plural = crate::naming::pluralize_snake(child_prefix);
    let collection_name = format!(
        "{}_{}_path",
        parent_singulars.join("_"),
        child_plural,
    );
    let parent_args = parent_ids.join(", ");
    let member_args = if parent_args.is_empty() {
        format!("{record_ref}.id")
    } else {
        format!("{parent_args}, {record_ref}.id")
    };
    format!(
        "if {record_ref}.id != 0 {{ route_helpers::{member_name}({member_args}) }} else {{ route_helpers::{collection_name}({parent_args}) }}",
    )
}

/// Emit a FormBuilder method call (`form.label("title", &opts)`,
/// `form.text_field("title", &article.title, &opts)`, …). Returns
/// `None` if the args can't be lowered to the runtime signature
/// (leaves the caller to fall through to the TODO placeholder).
///
/// The first positional arg, if a Sym, is the field name. Trailing
/// Hash literals become the options map. For text_field / textarea,
/// the current value is resolved from the form's bound record via
/// ctx.form_records — emit-side field access keeps the runtime
/// free of dynamic-dispatch on struct attributes.
/// Render a classified `ViewHelperKind` to a rust view-append
/// line. Mirrors TS's `emit_ts_view_helper`. Returns None when
/// arg shapes aren't renderable (caller falls through to the
/// TODO placeholder).
fn emit_rust_view_helper(
    kind: &crate::lower::ViewHelperKind<'_>,
    ctx: &ViewEmitCtx,
    buf_name: &str,
) -> Option<Vec<String>> {
    use crate::lower::ViewHelperKind::*;
    match kind {
        CsrfMetaTags => Some(vec![format!(
            "{buf_name}.push_str(&view_helpers::csrf_meta_tags());"
        )]),
        CspMetaTag => Some(vec![format!(
            "{buf_name}.push_str(&view_helpers::csp_meta_tag());"
        )]),
        JavascriptImportmapTags => Some(vec![format!(
            "{buf_name}.push_str(&view_helpers::javascript_importmap_tags(crate::importmap::PINS, \"application\"));"
        )]),
        TurboStreamFrom { channel } => {
            let chan = emit_view_string_arg(channel, ctx)?;
            Some(vec![format!(
                "{buf_name}.push_str(&view_helpers::turbo_stream_from(&{chan}));"
            )])
        }
        DomId { record, prefix } => {
            // Rust can't introspect record.type at runtime like JS
            // can. Derive the singular from the local's name — the
            // scaffold convention (`article`, `comment` as local
            // names) makes this reliable.
            let singular = match &*record.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name }
                    if ctx.is_local(name.as_str()) =>
                {
                    name.as_str().to_string()
                }
                ExprNode::Send {
                    recv: None,
                    method,
                    args,
                    block: None,
                    ..
                } if args.is_empty() && ctx.is_local(method.as_str()) => method.as_str().to_string(),
                _ => return None,
            };
            let id_expr = format!("{singular}.id");
            let prefix_expr = match prefix {
                None => "None".to_string(),
                Some(p) => match &*p.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => {
                        format!("Some({:?})", value.as_str())
                    }
                    ExprNode::Lit { value: Literal::Str { value } } => {
                        format!("Some({value:?})")
                    }
                    _ => "None".to_string(),
                },
            };
            Some(vec![format!(
                "{buf_name}.push_str(&view_helpers::dom_id({singular:?}, {id_expr}, {prefix_expr}));"
            )])
        }
        Pluralize { count, word } => {
            if !is_simple_view_expr(count, ctx) || !is_simple_view_expr(word, ctx) {
                return None;
            }
            let c = emit_view_expr_raw(count, ctx);
            let w = emit_view_expr_raw(word, ctx);
            Some(vec![format!(
                "{buf_name}.push_str(&view_helpers::pluralize({c} as i64, &{w}));"
            )])
        }
        Truncate { text, opts } => {
            if !is_simple_view_expr(text, ctx) {
                return None;
            }
            let t = emit_view_expr_raw(text, ctx);
            let opts_code = rust_opts_from_expr(opts.as_deref(), ctx);
            Some(vec![format!(
                "{buf_name}.push_str(&view_helpers::truncate(&{t}, &{opts_code}));"
            )])
        }
        StylesheetLinkTag { name, opts } => {
            let opts_code = rust_opts_from_expr(opts.as_deref(), ctx);
            // `:app` → one link per discovered stylesheet.
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*name.node {
                if value.as_str() == "app" && !ctx.stylesheets.is_empty() {
                    let mut out: Vec<String> = Vec::new();
                    for (i, n) in ctx.stylesheets.iter().enumerate() {
                        if i > 0 {
                            out.push(format!("{buf_name}.push_str(\"\\n\");"));
                        }
                        out.push(format!(
                            "{buf_name}.push_str(&view_helpers::stylesheet_link_tag({n:?}, &{opts_code}));"
                        ));
                    }
                    return Some(out);
                }
            }
            let name_str = match &*name.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => return None,
            };
            Some(vec![format!(
                "{buf_name}.push_str(&view_helpers::stylesheet_link_tag({name_str:?}, &{opts_code}));"
            )])
        }
        ContentForGetter { slot } => Some(vec![format!(
            "{buf_name}.push_str(&view_helpers::content_for_get({slot:?}));"
        )]),
        ContentForSetter { .. } => None,
        LinkTo { text, url, opts } => {
            let call = emit_rust_link_or_button("link_to", text, url, opts.as_deref(), ctx)?;
            Some(vec![format!("{buf_name}.push_str(&{call});")])
        }
        ButtonTo { text, target, opts } => {
            let call = emit_rust_link_or_button("button_to", text, target, opts.as_deref(), ctx)?;
            Some(vec![format!("{buf_name}.push_str(&{call});")])
        }
    }
}

/// Build a rust `HashMap<String, String>` expression from an
/// optional `Hash {...}` Expr representing helper opts. When the
/// opts expr is None or not a simple Hash, return an empty map
/// literal so the call still compiles.
fn rust_opts_from_expr(opts: Option<&Expr>, ctx: &ViewEmitCtx) -> String {
    match opts.map(|e| &*e.node) {
        Some(ExprNode::Hash { entries, .. }) => rust_hash_to_hashmap_literal(entries, ctx),
        _ => "std::collections::HashMap::<String, String>::new()".to_string(),
    }
}

/// Lower a link_to / button_to to a `view_helpers::{link_to,
/// button_to}(text, url, &opts)` call. Accepts the classifier's
/// separated fields (text/url/opts). URL resolution uses the
/// shared classifier.
fn emit_rust_link_or_button(
    helper: &str,
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewEmitCtx,
) -> Option<String> {
    if !is_simple_view_expr(text, ctx) {
        return None;
    }
    let text_expr = emit_view_expr_raw(text, ctx);
    let text_val = match &*text.node {
        ExprNode::Lit { value: Literal::Str { .. } } => text_expr,
        _ => format!("&{text_expr}.to_string()"),
    };
    let text_borrow = if text_val.starts_with('&') {
        text_val
    } else {
        format!("&{text_val}")
    };
    // URL expression — via the classifier.
    let is_local_fn = |n: &str| ctx.is_local(n);
    let url_kind = crate::lower::classify_view_url_arg(url, &is_local_fn)?;
    let url_expr = match url_kind {
        crate::lower::ViewUrlArg::Literal { value } => format!("{value:?}.to_string()"),
        crate::lower::ViewUrlArg::PathHelper { name, args } => {
            let args_s: Vec<String> = args
                .iter()
                .map(|a| rust_path_arg(a, ctx))
                .collect();
            format!("route_helpers::{name}({})", args_s.join(", "))
        }
        crate::lower::ViewUrlArg::RecordRef { name } => {
            format!("route_helpers::{name}_path({name}.id)")
        }
        crate::lower::ViewUrlArg::NestedArray { elements } => {
            rust_emit_nested_path(elements, ctx)?
        }
    };
    let opts_code = rust_opts_from_expr(opts, ctx);
    Some(format!(
        "view_helpers::{helper}({text_borrow}, &{url_expr}, &{opts_code})"
    ))
}

/// Lower a `[parent, child]` or `[parent.assoc, child]` array-URL
/// form to a nested-resource path-helper call. Returns None when
/// the element shapes aren't recognized.
fn rust_emit_nested_path(elements: &[Expr], ctx: &ViewEmitCtx) -> Option<String> {
    let is_local = |n: &str| ctx.is_local(n);
    let mut singulars: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for el in elements {
        let kind = crate::lower::classify_nested_url_element(el, &is_local)?;
        let (singular, id_expr) = rust_nested_element_parts(&kind);
        singulars.push(singular);
        args.push(id_expr);
    }
    let name = format!("{}_path", singulars.join("_"));
    Some(format!("route_helpers::{name}({})", args.join(", ")))
}

/// Render a classified `NestedUrlElement` to `(singular, id_expr)`
/// Rust source pair. Direct locals use `{name}.id`; belongs_to
/// reads use the owner's foreign-key column.
fn rust_nested_element_parts(kind: &crate::lower::NestedUrlElement<'_>) -> (String, String) {
    match kind {
        crate::lower::NestedUrlElement::DirectLocal { name } => {
            ((*name).to_string(), format!("{name}.id"))
        }
        crate::lower::NestedUrlElement::Association { owner, assoc } => {
            ((*assoc).to_string(), format!("{owner}.{assoc}_id"))
        }
    }
}

/// Lower a path-helper positional arg. Model-typed locals get
/// `.id` appended; numeric literals pass through.
fn rust_path_arg(arg: &Expr, ctx: &ViewEmitCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if ctx.is_local(name.as_str()) => {
            format!("{}.id", name.as_str())
        }
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && ctx.is_local(method.as_str()) =>
        {
            format!("{}.id", method.as_str())
        }
        _ => emit_view_expr_raw(arg, ctx),
    }
}

fn emit_rust_form_builder_call(
    recv: &str,
    rs_method: &str,
    args: &[Expr],
    ctx: &ViewEmitCtx,
) -> Option<String> {
    let (field, opts_entries) = crate::lower::classify_form_builder_args(args);
    let field = field.map(str::to_string);
    let opts = opts_entries
        .map(|entries| rust_hash_to_hashmap_literal(entries, ctx))
        .unwrap_or_else(|| "std::collections::HashMap::new()".to_string());
    match rs_method {
        "label" => {
            let field = field?;
            Some(format!("{recv}.label({field:?}, &{opts})"))
        }
        "text_field" | "textarea" => {
            let field = field?;
            // Resolve the bound record's field value. When the record
            // is a bare local with a known attribute list (the
            // common case: `form_with model: @article` + `form.text_
            // field :title` where `article` has a `title` attr), emit
            // `&article.title`. When the record is an expression we
            // can't type-check (degraded array-literal model, or a
            // field that's not an Article attr — like the show page's
            // Comment form trying `form.text_field :commenter` with
            // arg_name falling back to `article`), pass "" so the
            // generated code still compiles.
            let value_expr = ctx
                .form_records
                .iter()
                .find(|(name, _)| name == recv)
                .and_then(|(_, record)| {
                    if ctx.local_has_attr(record, &field) {
                        Some(format!("&{record}.{field}"))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "\"\"".to_string());
            Some(format!(
                "{recv}.{rs_method}({field:?}, {value_expr}, &{opts})",
            ))
        }
        "submit" => {
            // `form.submit "Add Comment", class: "..."` — first
            // positional arg is the custom label. Inject it into
            // opts under the "label" key so the runtime picks it up
            // instead of the scaffold "Create/Update {resource}"
            // default.
            let label_str = args.iter().find_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                _ => None,
            });
            let opts_expr = if let Some(lbl) = label_str {
                format!(
                    "{{ let mut __m = {opts}; __m.insert(\"label\".to_string(), {lbl:?}.to_string()); __m }}",
                )
            } else {
                opts
            };
            Some(format!("{recv}.submit(&{opts_expr})"))
        }
        _ => None,
    }
}

/// Lower a Ruby hash literal to a Rust `HashMap<String, String>`
/// expression. `data:` subhash flattens to `data-*` keys (Rails
/// convention); `class:` array with `{cond_class: cond_expr}` hash
/// expands to a conditional concatenation using
/// `view_helpers::field_has_error`.
fn rust_hash_to_hashmap_literal(
    entries: &[(Expr, Expr)],
    ctx: &ViewEmitCtx,
) -> String {
    let mut parts: Vec<(String, String)> = Vec::new();
    for (k, v) in entries {
        let key = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => continue,
        };
        // `data: { turbo_confirm: "..." }` → flatten to
        // `"data-turbo-confirm": "..."` entries.
        if key == "data" {
            if let ExprNode::Hash { entries: data_entries, .. } = &*v.node {
                for (dk, dv) in data_entries {
                    let dk_str = match &*dk.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            value.as_str().replace('_', "-")
                        }
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            value.replace('_', "-")
                        }
                        _ => continue,
                    };
                    let dv_s = match &*dv.node {
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            format!("{value:?}.to_string()")
                        }
                        _ if is_simple_view_expr(dv, ctx) => {
                            format!("{}.to_string()", emit_view_expr_raw(dv, ctx))
                        }
                        _ => continue,
                    };
                    parts.push((format!("data-{dk_str}"), dv_s));
                }
                continue;
            }
        }
        let val = if key == "class" {
            rust_class_value(v, ctx)
        } else {
            match &*v.node {
                ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}.to_string()"),
                ExprNode::Lit { value: Literal::Int { value } } => {
                    format!("{value}.to_string()")
                }
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    format!("{:?}.to_string()", value.as_str())
                }
                ExprNode::Array { elements, .. } => match elements.first() {
                    Some(first) => match &*first.node {
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            format!("{value:?}.to_string()")
                        }
                        _ => "String::new()".to_string(),
                    },
                    None => "String::new()".to_string(),
                },
                _ if is_simple_view_expr(v, ctx) => {
                    format!("{}.to_string()", emit_view_expr_raw(v, ctx))
                }
                _ => "String::new()".to_string(),
            }
        };
        parts.push((key, val));
    }
    let inserts: Vec<String> = parts
        .into_iter()
        .map(|(k, v)| format!("{{ let (k, v) = ({k:?}.to_string(), {v}); __m.insert(k, v); }}"))
        .collect();
    format!(
        "{{ let mut __m: std::collections::HashMap<String, String> = std::collections::HashMap::new(); {} __m }}",
        inserts.join(" "),
    )
}

/// Lower a `class:` option value. Handles three shapes:
///   - Simple (literal/interp): emit as-is
///   - `[base_string, {cond_class: cond_expr, ...}]`: emit a Rust
///     conditional-string expression via the shared class-value
///     classifier
///   - Anything else: `String::new()` placeholder
fn rust_class_value(v: &Expr, ctx: &ViewEmitCtx) -> String {
    let is_local = |n: &str| ctx.is_local(n);
    let simple_as_string = |e: &Expr| -> Option<String> {
        if is_simple_view_expr(e, ctx) {
            Some(match &*e.node {
                ExprNode::Lit { value: Literal::Str { value } } => {
                    format!("{value:?}.to_string()")
                }
                _ => format!("{}.to_string()", emit_view_expr_raw(e, ctx)),
            })
        } else {
            None
        }
    };
    match crate::lower::classify_class_value(v, &is_local) {
        crate::lower::ClassValueShape::Simple { expr } => {
            simple_as_string(expr).unwrap_or_else(|| "String::new()".to_string())
        }
        crate::lower::ClassValueShape::Conditional { base, clauses } => {
            let Some(base_s) = simple_as_string(base) else {
                return "String::new()".to_string();
            };
            if clauses.is_empty() {
                return base_s;
            }
            let extras: Vec<String> = clauses
                .iter()
                .map(|(cls_text, pred)| {
                    let cond_rs = rust_render_errors_field_predicate(pred);
                    format!(" + if {cond_rs} {{ \" {cls_text}\" }} else {{ \"\" }}")
                })
                .collect();
            format!("({base_s}{})", extras.join(""))
        }
        crate::lower::ClassValueShape::Unknown => "String::new()".to_string(),
    }
}

/// Render a classified errors-field predicate to a Rust boolean
/// expression via `view_helpers::field_has_error`.
fn rust_render_errors_field_predicate(pred: &crate::lower::ErrorsFieldPredicate<'_>) -> String {
    let call = format!(
        "view_helpers::field_has_error(&{}.errors, {:?})",
        pred.record, pred.field,
    );
    if pred.expect_present {
        call
    } else {
        format!("!{call}")
    }
}

/// Emit a single string-typed arg. Returns None if the arg isn't a
/// literal or simple interpolation.
fn emit_view_string_arg(arg: &Expr, ctx: &ViewEmitCtx) -> Option<String> {
    if let ExprNode::Lit { value: Literal::Str { value } } = &*arg.node {
        return Some(format!("{value:?}"));
    }
    if is_simple_view_expr(arg, ctx) {
        return Some(format!("{}.to_string()", emit_view_expr_raw(arg, ctx)));
    }
    None
}

/// Extract a kwarg `key:` from the hash-as-last-arg that Ruby ingests
/// keyword args into. Returns the expression bound to the key, if
/// present. Used by form_with / content_for kwarg extraction.
fn extract_kwarg<'a>(args: &'a [Expr], key: &str) -> Option<&'a Expr> {
    for arg in args {
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            for (k, v) in entries {
                if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                    if value.as_str() == key {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

/// Emit a view-body expression as Rust. Non-simple expressions
/// (FormBuilder chains, `.errors` lookups, has_many collection
/// accessors, helpers-with-models-as-args, etc.) degrade to
/// `"".to_string()` so the surrounding code still compiles.
/// Faithful rendering of the degraded forms needs a fuller Rails
/// runtime port and is scoped to a later phase.
fn emit_view_expr(expr: &Expr, ctx: &ViewEmitCtx) -> String {
    // Container literals (Hash/Array) pass through to the raw
    // emitter — each element gets its own simple-check via the
    // recursive `emit_view_expr` call in the container arm.
    let container =
        matches!(&*expr.node, ExprNode::Hash { .. } | ExprNode::Array { .. });
    // `render @coll` is always expanded — the loop body calls the
    // partial's render fn, which is always a real symbol.
    let is_render_call = matches!(
        &*expr.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "render" && args.len() == 1
    );
    if !container && !is_render_call && !is_simple_view_expr(expr, ctx) {
        return "/* TODO ERB */ String::new()".to_string();
    }
    emit_view_expr_raw(expr, ctx)
}

/// Raw emit, bypasses the simple-check. Called recursively for
/// container elements and from callers that already verified
/// simplicity.
fn emit_view_expr_raw(expr: &Expr, ctx: &ViewEmitCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_view_send(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    let k_s = emit_view_expr(k, ctx);
                    let v_s = emit_view_expr(v, ctx);
                    format!("({k_s}.to_string(), {v_s}.to_string())")
                })
                .collect();
            format!(
                "std::collections::HashMap::<String, String>::from([{}])",
                parts.join(", "),
            )
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_view_expr(e, ctx)).collect();
            format!("vec![{}]", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Emit as Rust format! — matches Ruby's `"foo#{x}"` semantics.
            use crate::expr::InterpPart;
            let mut fmt = String::new();
            let mut args: Vec<String> = Vec::new();
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '{' {
                                fmt.push_str("{{");
                            } else if c == '}' {
                                fmt.push_str("}}");
                            } else {
                                fmt.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        fmt.push_str("{}");
                        args.push(emit_view_expr(expr, ctx));
                    }
                }
            }
            if args.is_empty() {
                format!("{fmt:?}.to_string()")
            } else {
                format!("format!({fmt:?}, {})", args.join(", "))
            }
        }
        _ => format!("/* TODO view expr {:?} */", std::mem::discriminant(&*expr.node)),
    }
}

fn emit_view_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: &ViewEmitCtx,
) -> String {
    // `render @coll` / `render @x.assoc` → expand to a for-loop call.
    if recv.is_none() && method == "render" && args.len() == 1 && block.is_none() {
        return emit_view_render_call(&args[0], ctx);
    }
    // Strip `?` / `!` from the tail of method names — Rust idents
    // don't accept them. `.any?` → `.any`, `.present?` → `.present`.
    // Rails' convention is these are predicate/bang methods; in our
    // stub runtime they're implemented as plain methods.
    let sanitized_method = sanitize_method_name(method);
    // Route-helper routing: `articles_path()` / `new_article_path(
    // article)` → `route_helpers::` with model args coerced to
    // `.id`.
    if recv.is_none()
        && block.is_none()
        && (method.ends_with("_path") || method.ends_with("_url"))
    {
        let name = if let Some(stem) = method.strip_suffix("_url") {
            format!("{stem}_path")
        } else {
            method.to_string()
        };
        let args_s: Vec<String> = args
            .iter()
            .map(|a| emit_view_url_arg(a, ctx))
            .collect();
        return format!("route_helpers::{name}({})", args_s.join(", "));
    }
    // Bare Send matching a local var → emit bare (the fn arg,
    // loop var, or content_for binding).
    if recv.is_none() && args.is_empty() && block.is_none() && ctx.is_local(method) {
        return method.to_string();
    }
    // Rails helpers (link_to, button_to, etc.) → view_helpers.
    if recv.is_none() && is_rails_view_helper(method) {
        let args_s: Vec<String> = args.iter().map(|a| emit_view_expr(a, ctx)).collect();
        return format!("view_helpers::{method}({})", args_s.join(", "));
    }
    // Instance method `form.label :title` → form.label(&"title").
    if let Some(r) = recv {
        if args.is_empty() && block.is_none() {
            // Bare `.to_s` → `.to_string()`.
            if method == "to_s" {
                return format!("{}.to_string()", emit_view_expr(r, ctx));
            }
            // has_many association read: `article.comments` →
            // inline filter query. Recognized by resolving the
            // local + method against the flattened has_many map.
            let owner_local = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name }
                    if ctx.is_local(name.as_str()) =>
                {
                    Some(name.as_str().to_string())
                }
                ExprNode::Send {
                    recv: None,
                    method: m,
                    args: ra,
                    block: None,
                    ..
                } if ra.is_empty() && ctx.is_local(m.as_str()) => Some(m.as_str().to_string()),
                _ => None,
            };
            if let Some(owner) = owner_local {
                if let Some((target_class, fk)) =
                    ctx.resolve_has_many_on_local(&owner, method)
                {
                    return format!(
                        "{target_class}::all().into_iter().filter(|__r| __r.{fk} == {owner}.id).collect::<Vec<_>>()",
                    );
                }
            }
            // Collection predicate / size on any receiver:
            //   `.any?` / `.present?`  → `!<recv>.is_empty()`
            //   `.none?` / `.empty?`   → `<recv>.is_empty()`
            //   `.size` / `.count` / `.length` → `<recv>.len() as i64`
            // Works for both direct locals (`article.errors.any?`)
            // and partial-scope Send locals.
            match method {
                "any?" | "present?" => {
                    let recv_s = emit_view_expr(r, ctx);
                    return format!("!{recv_s}.is_empty()");
                }
                "none?" | "empty?" => {
                    let recv_s = emit_view_expr(r, ctx);
                    return format!("{recv_s}.is_empty()");
                }
                "size" | "count" | "length" => {
                    let recv_s = emit_view_expr(r, ctx);
                    return format!("{recv_s}.len() as i64");
                }
                // Known zero-arg methods on runtime types: emit with
                // parens so Rust parses as a call, not a field read.
                "full_message" => {
                    let recv_s = emit_view_expr(r, ctx);
                    return format!("{recv_s}.full_message()");
                }
                _ => {}
            }
            // Attribute access or method call with no args.
            let recv_s = emit_view_expr(r, ctx);
            return format!("{recv_s}.{sanitized_method}");
        }
        let recv_s = emit_view_expr(r, ctx);
        let args_s: Vec<String> = args.iter().map(|a| emit_view_expr(a, ctx)).collect();
        return format!("{recv_s}.{sanitized_method}({})", args_s.join(", "));
    }
    // Bare fn call — assume view_helpers as fallback. If the method
    // doesn't exist there the emitted project fails to compile,
    // which is a signal to either add it or treat the method as a
    // local/instance call instead.
    let args_s: Vec<String> = args.iter().map(|a| emit_view_expr(a, ctx)).collect();
    if args_s.is_empty() {
        format!("view_helpers::{method}()")
    } else {
        format!("view_helpers::{method}({})", args_s.join(", "))
    }
}

/// Strip trailing `?` / `!` from a method name. Rails predicates
/// (`.any?`, `.present?`) and bangs (`.destroy!`) don't survive
/// Rust's identifier grammar; the stub runtime exposes the
/// sanitized names instead.
fn sanitize_method_name(name: &str) -> String {
    let name = name.trim_end_matches('?');
    let name = name.trim_end_matches('!');
    name.to_string()
}

/// Render an argument to a `*_path(...)` helper. Model-typed locals
/// get `.id` appended so the path helper's `i64` signature is
/// satisfied.
fn emit_view_url_arg(arg: &Expr, ctx: &ViewEmitCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            // If the local is a known model, pass `.id`. Without
            // reliable type info here we pattern-match by name —
            // good enough for the scaffold (e.g. `article`, `comment`
            // are the singulars we'd singularize to).
            let class = crate::naming::singularize_camelize(name.as_str());
            if ctx.known_models.iter().any(|m| m.as_str() == class) {
                format!("{}.id", name.as_str())
            } else {
                name.to_string()
            }
        }
        _ => emit_view_expr(arg, ctx),
    }
}

/// Expand `render <collection_expr>` into a String-returning block
/// that loops over the collection and calls the per-item partial.
///
/// Handles two common shapes:
///   - `render @articles` where `@articles` is a view-scope local
///     bound to `&[T]` — straight `for __r in articles`.
///   - `render @article.comments` where `.comments` is a has_many
///     association — expand to a `Comment::all()` + filter loop
///     since the model struct doesn't expose a field accessor.
///
/// Anything else degrades to an empty string; faithfully handling
/// arbitrary collection expressions is scoped to a later phase.
fn emit_view_render_call(arg: &Expr, ctx: &ViewEmitCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if ctx.is_local(name.as_str()) => {
            // `render @articles` — straight loop over the local.
            let singular = crate::naming::singularize(name.as_str());
            let partial_name = format!("render_{singular}");
            let coll = name.to_string();
            format!(
                "{{ let mut __s = String::new(); for __r in {coll} {{ __s.push_str(&{partial_name}(__r)); }} __s }}",
            )
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if args.is_empty()
                && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) =>
        {
            // `render @article.comments` — has_many collection.
            // Resolve the target model via singularize + known-models
            // check, then expand to `Comment::all().into_iter().
            // filter(|c| c.article_id == article.id)`.
            let assoc_plural = method.as_str();
            let target_class = crate::naming::singularize_camelize(assoc_plural);
            if !ctx.known_models.iter().any(|m| m.as_str() == target_class) {
                return "/* TODO ERB: render over unknown collection */ String::new()".to_string();
            }
            let parent_name = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.to_string(),
                _ => unreachable!(),
            };
            let parent_singular = crate::naming::singularize(
                &crate::naming::singularize(&parent_name),
            );
            let fk = format!("{parent_singular}_id");
            let singular = crate::naming::singularize(assoc_plural);
            let partial_name = format!("render_{singular}");
            format!(
                "{{ let mut __s = String::new(); for __r in {target_class}::all().into_iter().filter(|__c| __c.{fk} == {parent_name}.id) {{ __s.push_str(&{partial_name}(&__r)); }} __s }}",
            )
        }
        _ => "/* TODO ERB: render */ String::new()".to_string(),
    }
}

/// Well-known Rails view helpers that route to `view_helpers::`.
/// Unlisted names fall through to the default Send emit (which
/// assumes an instance method or user-defined function).
fn is_rails_view_helper(name: &str) -> bool {
    matches!(
        name,
        "link_to"
            | "button_to"
            | "content_for"
            | "form_with"
            | "turbo_stream_from"
            | "dom_id"
            | "pluralize"
            | "number_to_currency"
            | "truncate"
            | "time_ago_in_words"
            | "image_tag"
            | "yield"
    )
}




/// Phase 4d controller emit — produces axum-shaped free fns per
/// action. For each of Rails' seven standard RESTful actions
/// (index/show/new/edit/create/update/destroy), emit a template body
/// that wires the action to the model runtime + route helpers +
/// views. Non-standard actions collapse to a stub that returns 501
/// Not Implemented; the scaffold blog doesn't have any.
fn emit_controller_axum(
    controller: &Controller,
    app: &App,
    known_models: &[Symbol],
) -> EmittedFile {
    let controller_name = controller.name.0.as_str();
    let resource = crate::lower::resource_from_controller_name(controller_name);
    let model_class = crate::naming::singularize_camelize(&resource);
    let has_model = known_models
        .iter()
        .any(|m| m.as_str() == model_class);
    let parent = crate::lower::find_nested_parent(app, controller_name);
    let permitted = crate::lower::permitted_fields_for(controller, &resource)
        .unwrap_or_else(|| crate::lower::default_permitted_fields(app, &model_class));

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "#![allow(unused_imports, unused_variables, unused_mut)]").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use std::collections::HashMap;").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use axum::extract::{{Form, Path}};").unwrap();
    writeln!(s, "use axum::response::{{IntoResponse, Response}};").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "use crate::http::{{self, Params}};").unwrap();
    if has_model {
        writeln!(s, "use crate::models::*;").unwrap();
    }
    writeln!(s, "use crate::route_helpers;").unwrap();
    writeln!(s, "use crate::views;").unwrap();
    writeln!(s).unwrap();

    let (public_actions, _private_actions) = crate::lower::split_public_private(controller);
    for (i, action) in public_actions.iter().enumerate() {
        if i > 0 {
            writeln!(s).unwrap();
        }
        let la = crate::lower::lower_action(
            action.name.as_str(),
            &resource,
            &model_class,
            has_model,
            parent.as_ref(),
            &permitted,
        );
        emit_rust_action(&mut s, &la, &action.body, known_models, controller);
    }

    let filename = format!("src/controllers/{}.rs", snake_case(controller_name));
    EmittedFile { path: PathBuf::from(filename), content: s }
}

/// Render one LoweredAction as an axum handler. Rust-specific
/// shapes: `Path(id): Path<i64>` extractors on routes with `:id`,
/// `Form(form): Form<HashMap<...>>` on POST/PATCH, `Response`
/// (not `ActionResponse`) returned via `into_response()`.
/// Render one LoweredAction as an axum handler. Signature-derivation
/// keys off `la.name` (show/edit/update/destroy carry a Path-extracted
/// `id`; create/update also take a `Form(form)` extractor; nested
/// controllers prepend the parent's id). Body emission delegates to
/// the action walker — the scaffold templates are gone.
fn emit_rust_action(
    out: &mut String,
    la: &crate::lower::LoweredAction,
    body: &Expr,
    known_models: &[Symbol],
    controller: &Controller,
) {
    let name = la.name.as_str();
    let has_id_in_path = matches!(name, "show" | "edit" | "update" | "destroy");
    let has_form = matches!(name, "create" | "update");

    // Collect signature params first so we can collapse to
    // `pub async fn name() -> Response {` when there are none.
    let mut sig_params = String::new();
    emit_path_params(&mut sig_params, la.parent.as_ref(), has_id_in_path);
    if has_form {
        writeln!(&mut sig_params, "    Form(form): Form<HashMap<String, String>>,").unwrap();
    }
    if sig_params.is_empty() {
        writeln!(out, "pub async fn {name}() -> Response {{").unwrap();
    } else {
        writeln!(out, "pub async fn {name}(").unwrap();
        out.push_str(&sig_params);
        writeln!(out, ") -> Response {{").unwrap();
    }

    if !la.has_model {
        writeln!(out, "    http::html(String::new()).into_response()").unwrap();
        writeln!(out, "}}").unwrap();
        return;
    }

    use crate::lower::CtrlWalker;
    let normalized = crate::lower::normalize_action_body(controller, name, body);
    let mut emitter = RsEmitter {
        ctx: crate::lower::WalkCtx {
            known_models,
            model_class: la.model_class.as_str(),
            resource: la.resource.as_str(),
            parent: la.parent.as_ref(),
            permitted: &la.permitted,
            adapter: &crate::adapter::SqliteAdapter,
        },
        state: crate::lower::WalkState::new(),
    };
    let body_src = emitter.walk_action_body(&normalized);
    out.push_str(&body_src);
    writeln!(out, "}}").unwrap();
}

/// Rust's controller-body emitter. See `src/lower/controller_walk.rs`
/// for the trait's shared walker dispatch.
struct RsEmitter<'a> {
    ctx: crate::lower::WalkCtx<'a>,
    state: crate::lower::WalkState,
}

impl<'a> crate::lower::CtrlWalker<'a> for RsEmitter<'a> {
    fn ctx(&self) -> &crate::lower::WalkCtx<'a> { &self.ctx }
    fn state_mut(&mut self) -> &mut crate::lower::WalkState { &mut self.state }
    fn indent_unit(&self) -> &'static str { "    " }

    fn write_assign(&mut self, name: &str, value: &Expr, indent: &str, out: &mut String) {
        let rhs = self.render_expr(value);
        writeln!(out, "{indent}let mut {name} = {rhs};").unwrap();
    }

    fn write_create_expansion(
        &mut self,
        var_name: &str,
        class: &str,
        indent: &str,
        out: &mut String,
    ) {
        writeln!(out, "{indent}let p = Params::new(form);").unwrap();
        let keys = self.ctx.permitted.iter()
            .map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", ");
        writeln!(out, "{indent}let fields = p.expect({:?}, &[{keys}]);", self.ctx.resource).unwrap();
        writeln!(out, "{indent}let mut {var_name} = {class} {{").unwrap();
        if let Some(parent) = self.ctx.parent {
            writeln!(out, "{indent}    {}_id,", parent.singular).unwrap();
        }
        for field in self.ctx.permitted {
            writeln!(
                out,
                "{indent}    {field}: fields.get({field:?}).cloned().unwrap_or_default(),",
            )
            .unwrap();
        }
        writeln!(out, "{indent}    ..Default::default()").unwrap();
        writeln!(out, "{indent}}};").unwrap();
    }

    fn write_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    ) {
        let cond_s = self.render_expr(cond);
        writeln!(out, "{indent}if {cond_s} {{").unwrap();
        self.walk_stmt(then_branch, out, depth + 1, is_tail);
        writeln!(out, "{indent}}} else {{").unwrap();
        if !crate::lower::is_empty_body(else_branch) {
            self.walk_stmt(else_branch, out, depth + 1, is_tail);
        }
        writeln!(out, "{indent}}}").unwrap();
    }

    fn write_update_if(
        &mut self,
        recv: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    ) {
        let recv_s = self.render_expr(recv);
        writeln!(out, "{indent}let p = Params::new(form);").unwrap();
        let keys = self.ctx.permitted.iter()
            .map(|k| format!("{k:?}")).collect::<Vec<_>>().join(", ");
        writeln!(out, "{indent}let fields = p.expect({:?}, &[{keys}]);", self.ctx.resource).unwrap();
        for field in self.ctx.permitted {
            writeln!(
                out,
                "{indent}if let Some(v) = fields.get({field:?}) {{ {recv_s}.{field} = v.clone(); }}",
            )
            .unwrap();
        }
        writeln!(out, "{indent}if {recv_s}.save() {{").unwrap();
        self.walk_stmt(then_branch, out, depth + 1, is_tail);
        writeln!(out, "{indent}}} else {{").unwrap();
        if !crate::lower::is_empty_body(else_branch) {
            self.walk_stmt(else_branch, out, depth + 1, is_tail);
        }
        writeln!(out, "{indent}}}").unwrap();
    }

    fn write_response_stmt(&mut self, r: &str, is_tail: bool, indent: &str, out: &mut String) {
        // Rust distinguishes trailing expression (no `return`/`;`)
        // from early-return. Only the function's final tail position
        // can use the bare-expression form; other positions need
        // an explicit `return`.
        if is_tail {
            writeln!(out, "{indent}{r}").unwrap();
        } else {
            writeln!(out, "{indent}return {r};").unwrap();
        }
    }

    fn write_expr_stmt(&mut self, s: &str, indent: &str, out: &mut String) {
        writeln!(out, "{indent}{s};").unwrap();
    }

    fn render_expr(&mut self, expr: &Expr) -> String {
        if let ExprNode::Send { recv, method, args, block, .. } = &*expr.node {
            if let Some(stmt) = self.render_send_stmt(
                recv.as_ref(), method.as_str(), args, block.as_ref(), "",
            ) {
                return match stmt {
                    crate::lower::Stmt::Response(r) => r,
                    crate::lower::Stmt::Expr(s) => s,
                };
            }
            let ctx_inner = EmitCtx {
                known_models: self.ctx.known_models,
                ..EmitCtx::default()
            };
            return emit_send(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx_inner);
        }
        let ctx_inner = EmitCtx {
            known_models: self.ctx.known_models,
            ..EmitCtx::default()
        };
        emit_expr(expr, ctx_inner)
    }

    fn render_send_stmt(
        &mut self,
        recv: Option<&Expr>,
        method: &str,
        args: &[Expr],
        block: Option<&Expr>,
        // Rust async consumption not wired yet; prefix unused for now.
        _suspending_prefix: &str,
    ) -> Option<crate::lower::Stmt> {
        use crate::lower::{SendKind, Stmt};
        let kind = crate::lower::classify_controller_send(
            recv, method, args, block, self.ctx.known_models,
        )?;
        Some(match kind {
            // params surface: `id` / `article_id` come in via axum's
            // Path extractor; render the bare local name.
            SendKind::ParamsIndex { key } => {
                let s = match &*key.node {
                    ExprNode::Lit { value: Literal::Sym { value: s } } => s.as_str().to_string(),
                    _ => self.render_expr(key),
                };
                Stmt::Expr(s)
            }
            SendKind::ParamsExpect { args: pe_args } => {
                let s = match pe_args.first().map(|e| &*e.node) {
                    Some(ExprNode::Lit { value: Literal::Sym { value: s } }) => s.as_str().to_string(),
                    _ => "todo!(\"params.expect hash\")".to_string(),
                };
                Stmt::Expr(s)
            }
            SendKind::ParamsAccess => Stmt::Expr("todo!(\"bare params\")".to_string()),

            SendKind::ModelNew { class } => {
                Stmt::Expr(format!("{}::default()", class.as_str()))
            }
            SendKind::ModelFind { class, id } => {
                let id_s = self.render_expr(id);
                Stmt::Expr(format!("{}::find({id_s}).unwrap_or_default()", class.as_str()))
            }
            SendKind::QueryChain { target: Some(target), method, args, recv } => {
                // Walk the chain bottom-up, then compose modifiers
                // onto `{Target}::all()`. `order`/`limit` produce
                // observable output; `includes`/`preload`/etc are
                // no-ops for sqlite.
                let modifiers = crate::lower::collect_chain_modifiers(method, args, recv.as_deref());
                let mut s = format!("{}::all()", target.as_str());
                for m in modifiers {
                    s = apply_rust_chain_modifier(s, m);
                }
                Stmt::Expr(s)
            }
            SendKind::QueryChain { target: None, .. } => {
                Stmt::Expr("todo!(\"unresolved query chain\")".to_string())
            }
            SendKind::AssocLookup { target, outer_method } => match outer_method {
                "find" => {
                    let id_s = args.first().map(|a| self.render_expr(a))
                        .unwrap_or_else(|| "0".to_string());
                    Stmt::Expr(format!(
                        "{}::find({id_s}).unwrap_or_default()", target.as_str(),
                    ))
                }
                _ => Stmt::Expr(format!("{}::default()", target.as_str())),
            },
            SendKind::BangStrip { recv, stripped_method, args: bs_args } => {
                let recv_s = self.render_expr(recv);
                let args_s: Vec<String> =
                    bs_args.iter().map(|a| self.render_expr(a)).collect();
                Stmt::Expr(format!("{recv_s}.{stripped_method}({})", args_s.join(", ")))
            }
            SendKind::InstanceUpdate => Stmt::Expr("false".to_string()),
            SendKind::PathOrUrlHelper => {
                let helper = method.strip_suffix("_url").or_else(|| method.strip_suffix("_path"))
                    .unwrap_or(method);
                Stmt::Expr(format!("route_helpers::{helper}_path()"))
            }
            SendKind::Render { args: r_args } => Stmt::Response(self.render_rs_render(r_args)),
            SendKind::RedirectTo { args: r_args } => Stmt::Response(self.render_rs_redirect(r_args)),
            SendKind::Head { args: h_args } => {
                let status = h_args.first().and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Sym { value: s } } =>
                        Some(crate::lower::status_sym_to_code(s.as_str())),
                    ExprNode::Lit { value: Literal::Int { value: n } } => Some(*n as u16),
                    _ => None,
                }).unwrap_or(200);
                Stmt::Response(format!(
                    "(axum::http::StatusCode::from_u16({status}).unwrap(), \"\").into_response()"
                ))
            }
            SendKind::RespondToBlock { .. }
            | SendKind::FormatHtml { .. }
            | SendKind::FormatJson => Stmt::Expr(
                "Response::default() /* unreachable: respond_to not normalized */".to_string(),
            ),
        })
    }
}

impl<'a> RsEmitter<'a> {
    fn render_rs_render(&mut self, args: &[Expr]) -> String {
        if let Some(first) = args.first() {
            if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*first.node {
                let suffix = sym.as_str();
                let plural = crate::naming::pluralize_snake(self.ctx.model_class);
                let view_fn = format!("{plural}_{suffix}");
                let arg = self.state.last_local.clone()
                    .unwrap_or_else(|| "&Default::default()".to_string());
                let wrapper = match crate::lower::extract_status_from_kwargs(&args[1..]) {
                    Some(422) => "http::unprocessable",
                    _ => "http::html",
                };
                return format!("{wrapper}(views::{view_fn}(&{arg})).into_response()");
            }
            let body_s = self.render_expr(first);
            return format!("http::html({body_s}).into_response()");
        }
        "http::html(String::new()).into_response()".to_string()
    }

    fn render_rs_redirect(&mut self, args: &[Expr]) -> String {
        let Some(first) = args.first() else {
            return "http::redirect(\"/\").into_response()".to_string();
        };
        let loc = self.render_expr(first);
        if is_bare_rs_ident(&loc) {
            let id_access = format!("{loc}.id");
            return format!(
                "http::redirect(&route_helpers::{loc}_path({id_access})).into_response()",
            );
        }
        if loc.starts_with("route_helpers::") {
            return format!("http::redirect(&{loc}).into_response()");
        }
        format!("http::redirect(&{loc}).into_response()")
    }
}

fn is_bare_rs_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() { return false; }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first == b'_') { return false; }
    bytes.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}


/// Emit the axum Path extractor(s) for an action. `with_id` adds the
/// leaf `:id` param; nested routes always include the parent.
fn emit_path_params(out: &mut String, parent: Option<&crate::lower::NestedParent>, with_id: bool) {
    match (parent, with_id) {
        (Some(parent), true) => {
            writeln!(
                out,
                "    Path(({}_id, id)): Path<(i64, i64)>,",
                parent.singular,
            )
            .unwrap();
        }
        (Some(parent), false) => {
            writeln!(out, "    Path({}_id): Path<i64>,", parent.singular).unwrap();
        }
        (None, true) => {
            writeln!(out, "    Path(id): Path<i64>,").unwrap();
        }
        (None, false) => {}
    }
}


/// Context threaded through expression emission. Grows as emission
/// shapes demand more information at leaf sites.
///
/// - `self_methods`: names of attributes/methods on the enclosing
///   `Self` class. Bare-name Sends matching one emit as `self.method`
///   rather than a bare identifier. Populated inside model methods.
/// - `in_test`: true when emitting a test-body expression. Enables
///   fixture accessors (`articles(:one)` → `fixtures::articles::one()`),
///   `Class.new(hash)` → struct-literal, and assertion mapping.
/// - `fixture_names`: fixture module names available in test scope
///   (e.g. `articles`, `comments`). Only consulted when `in_test`.
/// - `known_models`: names of emitted model classes. Used to decide
///   whether `Class.new(hash)` is a model constructor to be rendered
///   as a struct literal. Only consulted when `in_test`.
#[derive(Default, Clone, Copy)]
struct EmitCtx<'a> {
    self_methods: &'a [Symbol],
    in_test: bool,
    /// Enables Phase 4c controller-body Send rewrites: bare `params`,
    /// `params.expect`, `respond_to` + `format.html/json`, bare
    /// `redirect_to`/`render`/`head`, and `x.destroy!` → `x.destroy()`.
    in_controller: bool,
    fixture_names: &'a [Symbol],
    known_models: &'a [Symbol],
    /// Union of attribute names across every emitted model. Used as a
    /// fallback for the field-access heuristic when the receiver's type
    /// annotation isn't populated (the analyzer doesn't walk test
    /// bodies today; a future pass could remove this fallback).
    model_attrs: &'a [Symbol],
    /// `Some` when test-body emission needs to consult cross-model
    /// associations (`owner.assoc.create(hash)` rewrite). Defaults to
    /// `None` for emit paths outside tests.
    app: Option<&'a App>,
}

fn emit_body(body: &Expr, ctx: EmitCtx) -> String {
    // An action body like `@posts = Post.all` drops the ivar assignment and
    // just returns the RHS (Rails convention: ivars pass data to the view).
    // Local-variable assignments become `let` bindings. Multi-statement
    // bodies join with newlines, tail-expression is the function's return.
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, value } => emit_expr(value, ctx),
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                lines.push(emit_stmt(e, i == exprs.len() - 1, ctx));
            }
            lines.join("\n")
        }
        _ => emit_expr(body, ctx),
    }
}

fn emit_stmt(e: &Expr, is_last: bool, ctx: EmitCtx) -> String {
    match &*e.node {
        // Local `foo = expr` -> `let foo = expr;`.
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            if ctx.in_test {
                // Save/destroy take `&mut self`; mark every local as
                // mut so test bindings work uniformly. The allow-attr
                // on each test fn swallows the resulting unused-mut
                // warnings.
                format!("let mut {} = {};", name, emit_expr(value, ctx))
            } else {
                format!("let {} = {};", name, emit_expr(value, ctx))
            }
        }
        // Ivars in a multi-statement body: treat as locals. Later stmts can
        // read them via `ExprNode::Ivar` which also emits as the bare name.
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            if ctx.in_test {
                // Save/destroy take `&mut self`; mark every local as
                // mut so test bindings work uniformly. The allow-attr
                // on each test fn swallows the resulting unused-mut
                // warnings.
                format!("let mut {} = {};", name, emit_expr(value, ctx))
            } else {
                format!("let {} = {};", name, emit_expr(value, ctx))
            }
        }
        _ => {
            if is_last {
                emit_expr(e, ctx)
            } else {
                format!("{};", emit_expr(e, ctx))
            }
        }
    }
}

fn emit_expr(e: &Expr, ctx: EmitCtx) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        // Rails ivars become plain locals in the action body's Rust scope.
        // Cross-action ivar handoff (via filters, views) is a separate concern.
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Hash { entries, .. } => {
            // Rough approximation — real target code would probably want a
            // strongly-typed struct. HashMap::from is the dumbest-that-works.
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("({}, {})", emit_expr(k, ctx), emit_expr(v, ctx)))
                .collect();
            format!("HashMap::from([{}])", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_expr(e, ctx)).collect();
            format!("vec![{}]", parts.join(", "))
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(left, ctx), op_s, emit_expr(right, ctx))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
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
                        args.push(emit_expr(expr, ctx));
                    }
                }
            }
            if args.is_empty() {
                format!("{fmt:?}.to_string()")
            } else {
                format!("format!({fmt:?}, {})", args.join(", "))
            }
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value, ctx),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(|e| emit_expr(e, ctx)).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond, ctx);
            let then_s = emit_block_body(then_branch, ctx);
            let else_s = emit_block_body(else_branch, ctx);
            format!("if {cond_s} {{\n{then_s}\n}} else {{\n{else_s}\n}}")
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

/// Emit an expression as the body of a `{ ... }` block, indented one level.
/// For a Seq, each non-tail statement gets a trailing `;`; the tail stays
/// as the block's value expression. For a single expression, emit it alone.
/// Ruby blocks lower to `ExprNode::Lambda` in the IR, so peel one Lambda
/// layer and emit its body — callers treat Ruby `do ... end` as block
/// statements, not as closures.
fn emit_block_body(e: &Expr, ctx: EmitCtx) -> String {
    let inner = match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    };
    let raw = match &*inner.node {
        ExprNode::Seq { exprs } => {
            let mut lines: Vec<String> = Vec::new();
            for (i, stmt) in exprs.iter().enumerate() {
                if i == exprs.len() - 1 {
                    lines.push(emit_expr(stmt, ctx));
                } else {
                    lines.push(format!("{};", emit_expr(stmt, ctx)));
                }
            }
            lines.join("\n")
        }
        _ => emit_expr(inner, ctx),
    };
    raw.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: EmitCtx,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_expr(a, ctx)).collect();

    // Controller-scope rewrites check first so `params[:id]` doesn't
    // fall through to the generic `recv[args]` sugar (Params has no
    // Index impl).
    if ctx.in_controller {
        if let Some(s) = emit_controller_send(recv, method, args, &args_s, block, ctx) {
            return s;
        }
    }

    // `recv[args]` sugar for the `[]` method.
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap(), ctx), args_s.join(", "));
    }

    // Test-scope rewrites, only when ctx.in_test:
    //   - `articles(:one)` bare Send → `fixtures::articles::one()`
    //   - `assert_equal a, b` → `assert_eq!(a, b)`
    //   - `assert_not x` → `assert!(!x)`
    //   - `assert_not_nil x` → type-aware truthiness check
    //   - `Class.new(hash)` where Class is a known model → struct literal
    //   - `owner.assoc.{build,create}(hash)` → struct-literal rewrite
    //   - `assert_difference(expr, delta) { body }` → inline before/after
    if ctx.in_test {
        if let Some(s) = emit_test_send(recv, method, args, &args_s, block, ctx) {
            return s;
        }
    }

    match recv {
        None => {
            // Bare-name Send on implicit self: if the enclosing class has
            // this method/attribute, emit as `self.method` — Ruby's
            // self-dispatch doesn't translate to Rust's bare-name scope.
            // Controllers use `Self::name(...)` (associated fn, no &self)
            // since actions and private helpers are zero-arg free fns.
            if ctx.self_methods.iter().any(|s| s.as_str() == method) {
                if ctx.in_controller {
                    if args_s.is_empty() {
                        return format!("Self::{method}()");
                    } else {
                        return format!("Self::{method}({})", args_s.join(", "));
                    }
                }
                if args_s.is_empty() {
                    return format!("self.{method}");
                } else {
                    return format!("self.{method}({})", args_s.join(", "));
                }
            }
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r, ctx);
            // Class method dispatch (`Post.all`) uses `::`; instance method
            // dispatch (`post.title`) uses `.`. Heuristic: constants look
            // class-ish.
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            let sep = if is_class_call { "::" } else { "." };

            // Zero-arg Send on a model struct: a Ruby attribute read like
            // `comment.article_id` is a method call in Ruby but a plain
            // field access in Rust. Emit without parens when:
            //   - recv.ty is Ty::Class (authoritative), OR
            //   - method name matches a known model attribute (fallback
            //     for when the analyzer hasn't populated recv.ty, which
            //     today is the case for test bodies)
            // AND the method isn't a known AR/predicate name.
            if !is_class_call && args_s.is_empty() && !is_known_model_method(method) {
                let recv_is_model = matches!(&r.ty, Some(Ty::Class { .. }));
                let matches_attr = ctx.model_attrs.iter().any(|s| s.as_str() == method);
                if recv_is_model || matches_attr {
                    return format!("{recv_s}.{method}");
                }
            }

            // Ruby→Rust instance-method name mapping (e.g., `String#strip`
            // → `str::trim` + `.to_string()` so the return type stays
            // `String`). Only applies to instance dispatch — class calls
            // keep the Ruby name untouched.
            let (rust_method, suffix) = if is_class_call {
                (method.to_string(), String::new())
            } else {
                map_instance_method(method, r.ty.as_ref())
            };
            if args_s.is_empty() {
                format!("{recv_s}{sep}{rust_method}(){suffix}")
            } else {
                format!("{recv_s}{sep}{rust_method}({}){suffix}", args_s.join(", "))
            }
        }
    }
}

/// Test-scope Send rewrites. Returns `Some(rendered)` when a rule
/// applies, `None` to fall through to the normal emit paths.
fn emit_test_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    args_s: &[String],
    block: Option<&Expr>,
    ctx: EmitCtx,
) -> Option<String> {
    // `assert_difference("Class.count", delta) { body }` — measures the
    // named count expression before and after the block, asserts the
    // delta matches. Rails' convention is a string holding the Ruby
    // expression to re-evaluate; we parse the common `Class.count`
    // shape into `Class::count()` at emit time.
    if recv.is_none() && method == "assert_difference" {
        if let Some(body) = block {
            if let Some(count_expr) = args
                .first()
                .and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => rewrite_ruby_dot_call(value),
                    _ => None,
                })
            {
                let delta = args_s.get(1).cloned().unwrap_or_else(|| "1_i64".into());
                let body_s = emit_block_body(body, ctx);
                return Some(format!(
                    "{{\n            let _before = {count_expr};\n            {{\n{body_s}\n            }}\n            let _after = {count_expr};\n            assert_eq!(_after - _before, {delta})\n        }}",
                ));
            }
        }
    }
    // `articles(:one)` → `fixtures::articles::one()`
    if recv.is_none()
        && args.len() == 1
        && ctx.fixture_names.iter().any(|s| s.as_str() == method)
    {
        if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*args[0].node {
            return Some(format!("fixtures::{}::{}()", method, sym.as_str()));
        }
    }

    // Assertion macros.
    if recv.is_none() {
        match (method, args_s.len()) {
            ("assert_equal", 2) => {
                return Some(format!("assert_eq!({}, {})", args_s[0], args_s[1]));
            }
            ("assert_not_equal", 2) => {
                return Some(format!("assert_ne!({}, {})", args_s[0], args_s[1]));
            }
            ("assert_not", 1) => {
                return Some(format!("assert!(!{})", args_s[0]));
            }
            ("assert", 1) => {
                return Some(format!("assert!({})", args_s[0]));
            }
            ("assert_nil", 1) => {
                return Some(emit_assert_nil(&args_s[0], args[0].ty.as_ref(), false));
            }
            ("assert_not_nil", 1) => {
                return Some(emit_assert_nil(&args_s[0], args[0].ty.as_ref(), true));
            }
            _ => {}
        }
    }

    // `owner.<assoc>.create(hash)` / `.build(hash)` — HasMany association
    // surface. Rewrite to a struct-literal of the target model with the
    // foreign key pre-filled from `owner.id`, plus `save()` for the
    // `create` variant. No runtime association proxy required.
    if (method == "create" || method == "build") && args.len() == 1 {
        if let Some(outer_recv) = recv {
            if let ExprNode::Send { recv: Some(assoc_recv), method: assoc_method, args: inner_args, .. } = &*outer_recv.node {
                if inner_args.is_empty() {
                    if let Some(s) = try_emit_assoc_create(
                        assoc_recv,
                        assoc_method.as_str(),
                        args,
                        method,
                        ctx,
                    ) {
                        return Some(s);
                    }
                }
            }
        }
    }

    // `Class.new(hash)` → struct literal when Class is a known model.
    if let Some(r) = recv {
        if method == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class_name) = path.last() {
                    if ctx.known_models.iter().any(|s| s == class_name) {
                        if let ExprNode::Hash { entries, .. } = &*args[0].node {
                            let mut fields: Vec<String> = Vec::new();
                            for (k, v) in entries {
                                if let ExprNode::Lit {
                                    value: Literal::Sym { value: field_name },
                                } = &*k.node
                                {
                                    fields.push(format!(
                                        "            {}: {},",
                                        field_name.as_str(),
                                        emit_expr(v, ctx)
                                    ));
                                }
                            }
                            return Some(format!(
                                "{} {{\n{}\n            ..Default::default()\n        }}",
                                class_name,
                                fields.join("\n"),
                            ));
                        }
                    }
                }
            }
        }
    }

    None
}

/// Parse a Ruby-style `"Class.method"` expression string into Rust
/// `Class::method()` syntax, for use inside `assert_difference` and
/// similar helpers that take a string expression. Returns `None` for
/// shapes we don't handle; caller falls back to a TODO.
fn rewrite_ruby_dot_call(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    let (lhs, rhs) = trimmed.split_once('.')?;
    let is_ident = |s: &str| {
        !s.is_empty() && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if !is_ident(lhs) || !is_ident(rhs) {
        return None;
    }
    // Capitalized LHS looks like a class name → `Class::method()`.
    // Lowercase LHS looks like an instance → `lhs.method()`.
    let is_class = lhs.chars().next().is_some_and(|c| c.is_uppercase());
    if is_class {
        Some(format!("{lhs}::{rhs}()"))
    } else {
        Some(format!("{lhs}.{rhs}"))
    }
}

/// Controller-scope Send rewrites. Drives off the shared classifier
/// `lower::classify_controller_send`; every arm below is a
/// render-table entry from `SendKind` → Rust syntax. Returns `None`
/// when the classifier doesn't match (falls through to plain Send).
fn emit_controller_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    args_s: &[String],
    block: Option<&Expr>,
    ctx: EmitCtx,
) -> Option<String> {
    use crate::lower::SendKind;
    let kind =
        crate::lower::classify_controller_send(recv, method, args, block, ctx.known_models)?;
    Some(match kind {
        SendKind::ParamsAccess => "crate::http::params()".to_string(),

        SendKind::ParamsExpect { .. } => "todo!(\"params.expect\")".to_string(),

        SendKind::ParamsIndex { .. } => {
            let arg = args_s.first().cloned().unwrap_or_default();
            format!("crate::http::params().expect({arg})")
        }

        SendKind::ModelNew { class } => format!("{}::default()", class.as_str()),

        SendKind::ModelFind { class, .. } => {
            let arg = args_s.first().cloned().unwrap_or_default();
            format!("{}::find({arg}).unwrap_or_default()", class.as_str())
        }

        SendKind::AssocLookup { target, .. } => format!("{}::default()", target.as_str()),

        SendKind::QueryChain { target: Some(target), .. } => {
            format!("Vec::<{}>::new()", target.as_str())
        }
        SendKind::QueryChain { target: None, .. } => "todo!(\"query chain\")".to_string(),

        SendKind::PathOrUrlHelper => "todo!(\"route helper\")".to_string(),

        SendKind::BangStrip { recv, stripped_method, .. } => {
            let recv_s = emit_expr(recv, ctx);
            format!("{recv_s}.{stripped_method}()")
        }

        SendKind::InstanceUpdate => "todo!(\"Model::update\")".to_string(),

        SendKind::Render { .. } => {
            let arg = args_tuple_or_single(args_s);
            format!("crate::http::render({arg})")
        }

        SendKind::RedirectTo { .. } => match args_s.len() {
            0 => "crate::http::redirect_to(())".to_string(),
            1 => format!("crate::http::redirect_to({})", args_s[0]),
            _ => format!(
                "crate::http::redirect_to_with({}, ({}))",
                args_s[0],
                args_s[1..].join(", "),
            ),
        },

        SendKind::Head { .. } => {
            let arg = args_tuple_or_single(args_s);
            format!("crate::http::head({arg})")
        }

        SendKind::RespondToBlock { body } => {
            let body_rendered = emit_respond_to_body(body, ctx);
            let indented = body_rendered
                .lines()
                .map(|l| format!("    {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "crate::http::respond_to(|__fr| {{\n{indented}\n    Response::default()\n}})"
            )
        }

        SendKind::FormatHtml { body } => {
            // Re-wrap in a synthetic expr so emit_block_body sees the
            // same shape it did under the old code path (a Seq or
            // single expr as the block body).
            let body_s = emit_block_body(body, ctx);
            format!("__fr.html(|| {{\n{body_s}\n}})")
        }

        SendKind::FormatJson => {
            "/* TODO: JSON branch (Phase 4e) */ Response::default()".to_string()
        }
    })
}

/// Render a `respond_to` block body. The body is usually an
/// `if article.save ... else ... end` with `format.html/.json` calls
/// in each branch. We emit each such call as a `;`-terminated
/// statement against `__fr` (for `.html`) or a TODO comment (for
/// `.json`), then let the outer emitter append `Response::default()`.
fn emit_respond_to_body(body: &Expr, ctx: EmitCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|e| format!("{};", emit_expr(e, ctx)))
            .collect::<Vec<_>>()
            .join("\n"),
        ExprNode::If { cond, then_branch, else_branch } => {
            // Recurse into branches so nested `format.*` calls still
            // get rewritten. Tail of the if-else is discarded; the
            // outer emitter emits `Response::default()` after.
            let cond_s = emit_expr(cond, ctx);
            let then_s = emit_respond_to_body(then_branch, ctx)
                .lines()
                .map(|l| format!("    {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            let else_s = emit_respond_to_body(else_branch, ctx)
                .lines()
                .map(|l| format!("    {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("if {cond_s} {{\n{then_s}\n}} else {{\n{else_s}\n}};")
        }
        _ => format!("{};", emit_expr(body, ctx)),
    }
}

/// When a `render`/`head` call has a single arg, pass it through as-is.
/// Multiple args pack into a tuple the stub's generic `T` accepts.
fn args_tuple_or_single(args_s: &[String]) -> String {
    match args_s.len() {
        0 => "()".to_string(),
        1 => args_s[0].clone(),
        _ => format!("({})", args_s.join(", ")),
    }
}

/// Rewrite `owner.<assoc>.create(hash)` / `.build(hash)` into a
/// struct-literal construction of the target model, with the foreign
/// key prefilled from `owner.id`. Returns `None` when the pattern
/// doesn't apply (e.g. we can't identify `<assoc>` as a HasMany on any
/// known model) so callers fall through to generic emission.
fn try_emit_assoc_create(
    owner: &Expr,
    assoc_name: &str,
    args: &[Expr],
    outer_method: &str,
    ctx: EmitCtx,
) -> Option<String> {
    let app = ctx.app?;
    let resolved = crate::lower::resolve_has_many(
        &Symbol::from(assoc_name),
        owner.ty.as_ref(),
        app,
    )?;
    let target_class = resolved.target_class.0.as_str();
    let foreign_key = resolved.foreign_key.as_str();

    let owner_s = emit_expr(owner, ctx);
    let hash_entries = match &args.first()?.node.as_ref() {
        ExprNode::Hash { entries, .. } => entries.clone(),
        _ => return None,
    };

    let mut field_lines: Vec<String> = Vec::new();
    field_lines.push(format!("                {foreign_key}: {owner_s}.id,"));
    for (k, v) in &hash_entries {
        if let ExprNode::Lit { value: Literal::Sym { value: field_name } } = &*k.node {
            field_lines.push(format!(
                "                {}: {},",
                field_name.as_str(),
                emit_expr(v, ctx),
            ));
        }
    }

    let struct_lit = format!(
        "{target_class} {{\n{}\n                ..Default::default()\n            }}",
        field_lines.join("\n"),
    );
    // `.build` returns the unsaved record; `.create` saves it first.
    // Both yield the record so tests can read `.article_id` etc. The
    // `let mut` is needed for the `save()` path; harmless for build.
    let body = if outer_method == "create" {
        format!(
            "{{\n            let mut r = {struct_lit};\n            r.save();\n            r\n        }}"
        )
    } else {
        format!("{{\n            let mut r = {struct_lit};\n            r\n        }}")
    };
    Some(body)
}

/// Render an `assert_not_nil` (or `assert_nil`) with type-aware truthiness.
/// Truthy side when `expect_present` is true (assert_not_nil); falsy side
/// otherwise (assert_nil). Ruby's `nil?` has no universal Rust equivalent —
/// the right check depends on the value's type.
fn emit_assert_nil(expr: &str, ty: Option<&Ty>, expect_present: bool) -> String {
    let (truthy, falsy) = match ty {
        Some(Ty::Int) | Some(Ty::Float) => (
            format!("{expr} != 0"),
            format!("{expr} == 0"),
        ),
        Some(Ty::Str) => (
            format!("!{expr}.is_empty()"),
            format!("{expr}.is_empty()"),
        ),
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            (format!("{expr}.is_some()"), format!("{expr}.is_none()"))
        }
        _ => (
            format!("/* TODO: assert_nil on unknown ty */ true"),
            format!("/* TODO: assert_nil on unknown ty */ false"),
        ),
    };
    if expect_present {
        format!("assert!({truthy})")
    } else {
        format!("assert!({falsy})")
    }
}

/// Methods that are calls with parens on an emitted model struct (not
/// attribute reads). Used by emit_send to decide whether a zero-arg
/// Send should render as `x.method()` or as `x.method` field access.
/// Custom user-defined methods on a specific model would ideally extend
/// this list; for now the AR core + predicates covers real-blog.
fn is_known_model_method(name: &str) -> bool {
    matches!(
        name,
        "save" | "save!" | "destroy" | "destroy!" | "update" | "update!"
            | "delete" | "touch" | "reload" | "valid?" | "invalid?"
            | "persisted?" | "new_record?" | "destroyed?" | "changed?"
            | "validate" | "attributes" | "errors"
    )
}

/// Map a Ruby instance-method call to its Rust equivalent, returning the
/// Rust method name and an optional call-site suffix (commonly `.to_string()`
/// when a `&str`-returning Rust method replaces a `String`-returning Ruby
/// one). Grows as new fixtures surface new cases; unmapped methods pass
/// through with their Ruby name.
fn map_instance_method(method: &str, recv_ty: Option<&Ty>) -> (String, String) {
    match recv_ty {
        Some(Ty::Str) => match method {
            // Ruby's `strip` returns a new String; Rust's `trim` returns
            // `&str`. Wrap in `.to_string()` so the type matches.
            "strip" => ("trim".into(), ".to_string()".into()),
            _ => (method.into(), String::new()),
        },
        _ => (method.into(), String::new()),
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

fn emit_literal(lit: &Literal) -> String {
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
