//! Go target — `go2` parallel emit (Phase 1 scaffold).
//!
//! Mirrors the `rust2` migration pattern. Strangler-fig orchestrator
//! that runs alongside the legacy `src/emit/go.rs` while the migration
//! to Group 1 (lowered IR + transpiled `runtime/ruby/`) lands.
//!
//! Selected at runtime via `ROUNDHOUSE_GO_V2=1`. Without the env var,
//! `super::go::emit` runs unchanged and emits the same output it
//! always has. With the env var, this module's overlay runs after
//! the legacy emit and writes transpiled framework runtime files
//! into the output project under `app/v2/` (separate Go package, so
//! emission can't conflict with the hand-written runtime types).
//!
//! Phase 1 scope: scaffold + minimal transpile via stub
//! `library::emit_library_class` (see `library.rs` for the contract).
//! Transpiled files are emitted but their method bodies are
//! `panic("go2 stub")` — `go build ./app/v2/...` produces a real
//! error inventory we can drive future sessions against.
//!
//! Out of scope for Phase 1: replacing the hand-written
//! `runtime/go/*.go` files (cable, http, server, view_helpers,
//! runtime, test_support, db) with transpiled equivalents. Those
//! land once the per-method body emit is real enough that calls
//! through them survive `go build`.

use std::path::PathBuf;

use super::EmittedFile;
use crate::App;

mod expr;
mod library;
pub mod lower;
mod ty;

/// Phase 3 hand-written primitive runtime. Verbatim copy into the
/// v2/ overlay — these files declare `package v2` already so no
/// rewriting needed. They land alongside the transpiled framework
/// runtime so cross-file references (the transpiled `ActiveRecord`
/// module-singleton's `*AdapterInterface` slot type) resolve at
/// `go vet` / `go build` time.
const RT_V2_ADAPTER_INTERFACE: &str =
    include_str!("../../runtime/go/v2/adapter_interface.go");
const RT_V2_FRAMEWORK_TEST_ADAPTER: &str =
    include_str!("../../runtime/go/v2/framework_test_adapter.go");
const RT_V2_PARAM_VALUE: &str =
    include_str!("../../runtime/go/v2/param_value.go");
const RT_V2_DB: &str =
    include_str!("../../runtime/go/v2/db.go");
const RT_V2_BROADCASTS: &str =
    include_str!("../../runtime/go/v2/broadcasts.go");
const RT_V2_SERVER: &str =
    include_str!("../../runtime/go/v2/server.go");
const RT_V2_ROUTER_GLUE: &str =
    include_str!("../../runtime/go/v2/router_glue.go");

/// Append go2 transpiled runtime files to `files` when
/// `ROUNDHOUSE_GO_V2=1`. No-op otherwise — the default emit pipeline
/// (legacy go) ships unchanged.
pub fn overlay_v2(files: &mut Vec<EmittedFile>, app: &App) {
    if std::env::var("ROUNDHOUSE_GO_V2").as_deref() != Ok("1") {
        return;
    }
    files.extend(emit_overlay_files(app));
}

/// Produce the v2 overlay files unconditionally — for tests and
/// other callers that want the overlay output without setting an env
/// var. Returns the same files `overlay_v2` would append, in
/// emission order.
pub fn emit_overlay_files(app: &App) -> Vec<EmittedFile> {
    let mut out = Vec::new();

    // Hand-written runtime — copied verbatim under `app/v2/`.
    // Emitted FIRST so the transpiled framework runtime files can
    // assume their types resolve at parse time.
    // Always-on overlay shims — stdlib-only imports, ship unconditionally
    // with `ROUNDHOUSE_GO_V2=1`. Model-only shims (db, broadcasts) ship
    // below behind the models env var so the default toolchain test
    // (which doesn't tidy go.mod for app-only sqlite dependencies)
    // stays clean.
    for (name, src) in [
        ("adapter_interface.go", RT_V2_ADAPTER_INTERFACE),
        ("framework_test_adapter.go", RT_V2_FRAMEWORK_TEST_ADAPTER),
        ("param_value.go", RT_V2_PARAM_VALUE),
    ] {
        out.push(EmittedFile {
            path: PathBuf::from(format!("app/v2/{name}")),
            content: src.to_string(),
        });
    }

    let units = match crate::runtime_loader::go_units(|_ns, classes| {
        lower::lower_for_go(classes)
    }) {
        Ok(u) => u,
        Err(e) => {
            // Transpile failure surfaces as a sentinel file rather
            // than panicking — `go build` picks it up and the cargo
            // test that exercises overlay sees a non-empty result.
            out.push(EmittedFile {
                path: PathBuf::from("app/v2/transpile_error.txt"),
                content: format!("go2 transpile failed: {e}\n"),
            });
            return out;
        }
    };

    for unit in units {
        // The runtime_loader produces paths shaped like `app/X.go`
        // from GO_RUNTIME; relocate everything under `app/v2/` and
        // re-anchor the package to `v2` so this overlay can never
        // collide with legacy runtime types of the same name
        // (Inflector, JsonBuilder, Router, ...).
        let file_name = unit
            .out_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unit.go".to_string());
        let out_path = PathBuf::from(format!("app/v2/{file_name}"));
        let content = rewrite_package_to_v2(&unit.content);
        out.push(EmittedFile {
            path: out_path,
            content,
        });
    }

    // Phase 3: app models → `app/v2/<snake>.go`. Inventory-mode for
    // now — env-gated so the toolchain test (which runs `go vet` over
    // the whole v2/ overlay) stays clean while the model-emit gaps
    // close one at a time. `ROUNDHOUSE_GO_V2_MODELS=1` opts in; the
    // default emit (and the existing inflector_v2_compiles_and_runs
    // smoke test) ships unchanged. Mirrors rust2's Phase 5 pattern.
    if std::env::var("ROUNDHOUSE_GO_V2_MODELS").as_deref() == Ok("1")
        && !app.models.is_empty()
    {
        // Model-only runtime shims. Db_* bridges database/sql for the
        // per-model adapter_emit lowerer's `Db.prepare(sql)` /
        // `Db.step?(stmt)` / `Db.column_*` calls; Broadcasts_* captures
        // the broadcasts_to-emitted `Broadcasts.prepend(...)` log.
        // Both have no useful Ruby implementation to transpile —
        // mirrors `runtime/rust/db.rs` + `runtime/rust/broadcasts.rs`
        // hand-written rationale. Gated here (not at the always-on
        // overlay above) because db.go pulls modernc.org/sqlite into
        // go.mod and the default toolchain test won't tidy.
        for (name, src) in [
            ("db.go", RT_V2_DB),
            ("broadcasts.go", RT_V2_BROADCASTS),
            // Phase 4 minimum wedge — server boot + empty-mux router
            // glue so the emitted main.go compiles. Per-route binding
            // lands in the next wedge.
            ("server.go", RT_V2_SERVER),
            ("router_glue.go", RT_V2_ROUTER_GLUE),
        ] {
            out.push(EmittedFile {
                path: PathBuf::from(format!("app/v2/{name}")),
                content: src.to_string(),
            });
        }

        // schema_sql.go — DDL constant the boot path passes to
        // OpenProductionDB. Reuses the target-neutral schema renderer
        // (same source the legacy go target consumes for its own
        // app/schema_sql.go).
        let ddl = crate::emit::shared::schema_sql::render_schema_sql(&app.schema);
        out.push(EmittedFile {
            path: PathBuf::from("app/v2/schema_sql.go"),
            content: format!(
                "// Generated by Roundhouse (go2).\npackage v2\n\nconst CreateTables = `\n{ddl}`\n",
            ),
        });

        // main.go — package-main binary entry, sibling to the legacy
        // `<root>/main.go`. Placed at `cmd/v2/main.go` so `go build
        // ./cmd/v2/` builds the v2 binary without colliding with the
        // legacy main.go's package main. Mirrors rust2's `src/main.rs`
        // template emit (wedge 2b).
        out.push(EmittedFile {
            path: PathBuf::from("cmd/v2/main.go"),
            content: "// Generated by Roundhouse (go2).\npackage main\n\n\
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
}\n".to_string(),
        });

        let model_lcs = crate::lower::model_to_library::lower_models_to_library_classes(
            &app.models,
            &app.schema,
            vec![],
        );
        let lowered = lower::lower_for_go(model_lcs);
        for lc in &lowered {
            let class_text = match library::emit_library_class(lc) {
                Ok(s) => s,
                Err(e) => format!("// emit error: {e}\n"),
            };
            // Wrap with `package app` so rewrite_package_to_v2 swaps
            // to `package v2` and injects per-file stdlib imports.
            let raw = format!("// Generated from app/models/{stem}.rb at app emit time.\n// Do not edit by hand — edit the source `.rb` and re-run emit.\n\npackage app\n\n{class_text}",
                stem = crate::naming::snake_case(lc.name.0.as_str()),
            );
            let content = rewrite_package_to_v2(&raw);
            let stem = crate::naming::snake_case(lc.name.0.as_str());
            out.push(EmittedFile {
                path: PathBuf::from(format!("app/v2/{stem}.go")),
                content,
            });
        }

        // Per-view stubs for the broadcasts_to-referenced partials.
        // The model lowerer's `broadcasts_to` expansion synthesizes
        // calls like `Views::Articles.article(self)` inside
        // `after_create_commit`. Without a concrete `Views::X` module
        // those references vet-fail (`undefined: ViewsArticles_article`).
        // Emit minimal stubs that produce an empty-string fragment —
        // sufficient for the live broadcast log to capture the action,
        // pending real view emit. Each stub is gated by
        // `app.views` containing a partial under the matching
        // resource dir; missing-dir → no stub (and the model emit's
        // broadcasts_to expansion presumably wouldn't reference it).
        // Mirrors the rust2 "view-module stubs from lowerer" pass.
        let referenced: std::collections::BTreeSet<(String, String)> =
            crate::lower::view_to_library::lower_views_to_library_classes(
                &app.views, app, vec![],
            )
            .into_iter()
            .flat_map(|lc| {
                let mod_name = sanitize_module_name(lc.name.0.as_str());
                lc.methods
                    .into_iter()
                    .filter(|m| matches!(m.receiver, crate::dialect::MethodReceiver::Class))
                    .map(move |m| (mod_name.clone(), m.name.as_str().to_string()))
            })
            .collect();
        if !referenced.is_empty() {
            let mut by_module: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            for (m, name) in referenced {
                by_module.entry(m).or_default().push(name);
            }
            for (module_name, methods) in by_module {
                let mut body = String::new();
                body.push_str("// View module stub. Real view emit lands in a later phase;\n");
                body.push_str("// the empty-string fallback keeps the broadcast log capturing\n");
                body.push_str("// the action while the html fragment shape isn't yet wired.\n\n");
                body.push_str("package app\n\n");
                for method in methods {
                    // Module-singleton emit shape from go2/library.rs:
                    // `func <ClassName>_<ruby_method_name>(...)`. Match
                    // it exactly so the model's `Views::X.partial(self)`
                    // calls resolve. Ruby method name is unmodified
                    // (no pascalization for the bare-fn suffix).
                    body.push_str(&format!(
                        "func {module_name}_{method}(_record interface{{}}) string {{\n\treturn \"\"\n}}\n\n",
                    ));
                }
                let content = rewrite_package_to_v2(&body);
                let stem = crate::naming::snake_case(&module_name);
                out.push(EmittedFile {
                    path: PathBuf::from(format!("app/v2/{stem}_stubs.go")),
                    content,
                });
            }
        }
    }

    out
}

/// Replace the leading `package app` declaration with `package v2`
/// and inject `import` declarations for stdlib packages the body
/// references (`cmp`, `fmt`, `regexp`, `slices`, `strings`, `time`).
/// Go is strict about unused imports so detection is by substring
/// presence, not always-on.
/// `Views::Articles` → `ViewsArticles`. Same logic as
/// `library.rs::sanitize_type_name`, repeated here so the stub-emit
/// path doesn't need to reach into library internals for a one-liner.
fn sanitize_module_name(name: &str) -> String {
    name.replace("::", "")
}

fn rewrite_package_to_v2(content: &str) -> String {
    let imports = needed_imports(content);
    let mut out = String::with_capacity(content.len() + 64);
    let mut saw_pkg = false;
    let mut imports_emitted = false;
    for line in content.lines() {
        if !saw_pkg && line.starts_with("package ") {
            out.push_str("package v2\n");
            saw_pkg = true;
        } else {
            // First non-package, non-comment line is where the imports
            // go. Pre-pending here keeps them above transpiled headers.
            if saw_pkg && !imports_emitted && !imports.is_empty()
                && !line.starts_with("//")
                && !line.trim().is_empty()
            {
                emit_imports(&mut out, &imports);
                imports_emitted = true;
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    // Fallthrough: file had only comments + package line. Still emit
    // imports if needed (the body might be empty, but that's harmless).
    if saw_pkg && !imports_emitted && !imports.is_empty() {
        emit_imports(&mut out, &imports);
    }
    if !saw_pkg {
        let mut prefixed = String::from("package v2\n\n");
        if !imports.is_empty() {
            emit_imports(&mut prefixed, &imports);
        }
        prefixed.push_str(&out);
        return prefixed;
    }
    out
}

fn needed_imports(content: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    // Each entry is `(probes[], package)`. A package gets imported
    // when ANY of its probes is present in `content`. Most stdlib
    // packages have a unique-enough top-level prefix (`fmt.`,
    // `strings.`, …) that the bare-period probe is safe; `time.` is
    // an exception because the file header comment "at app emit time."
    // false-matches, so its probes specifically target the call sites
    // the peepholes emit (`time.Now(`, `time.RFC3339`). Add probes
    // here when a new `time.X` emit lands.
    let entries: &[(&[&str], &str)] = &[
        (&["base64."], "encoding/base64"),
        (&["cmp."], "cmp"),
        (&["fmt."], "fmt"),
        (&["json."], "encoding/json"),
        (&["regexp."], "regexp"),
        (&["slices."], "slices"),
        (&["strings."], "strings"),
        (&["time.Now(", "time.RFC3339"], "time"),
    ];
    for (probes, name) in entries {
        if probes.iter().any(|p| content.contains(p)) {
            out.push(*name);
        }
    }
    out
}

fn emit_imports(out: &mut String, imports: &[&str]) {
    if imports.len() == 1 {
        out.push_str(&format!("import {:?}\n\n", imports[0]));
    } else {
        out.push_str("import (\n");
        for name in imports {
            out.push_str(&format!("\t{name:?}\n"));
        }
        out.push_str(")\n\n");
    }
}

// Re-export the library emit functions used by `runtime_loader::GO_TARGET`.
// `emit_library_class` is `pub` (not `pub(crate)`) so integration
// tests in `tests/` can synthesize a `LibraryClass` and assert the
// emitted shape without going through the GO_RUNTIME wiring — that
// keeps shape coverage decoupled from "is this file currently in
// the v2/ overlay" gating.
pub use library::emit_library_class;
pub(crate) use library::{emit_module, format_constant, format_module_ivar};
