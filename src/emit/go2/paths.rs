//! Output-path routing for the go2 emitter.
//!
//! Every emitted file in the go2 overlay flows through `output_path`
//! below. Phase 1 (#19) returns the current flat `app/v2/…` layout
//! unchanged; the indirection exists so Phase 4's cutover to
//! `internal/models/`, `pkg/runtime/<subpkg>/`, etc. is a focused diff
//! against this one function rather than the ~18 hardcoded path sites
//! that previously lived inline in `go2.rs`.
//!
//! See GitHub issue #19 for the target layout. See issue #20 for the
//! cross-model cycle work that must land before the cutover can be
//! correct for multi-model apps.

use std::path::PathBuf;

/// Logical kind of an emitted go2 file. Drives package + path
/// resolution. Adding a new emitted artifact means adding a variant
/// here and a match arm in `output_path` — the call site in
/// `go2.rs` then becomes a single `output_path(OutputKind::X { … })`
/// call, paying the cutover cost once.
pub(crate) enum OutputKind<'a> {
    /// Hand-written runtime shim, addressed by the basename of its
    /// source file under `runtime/go/` (e.g. `adapter_interface.go`,
    /// `slots.go`, `db.go`).
    HandWrittenRuntime { name: &'a str },

    /// Transpiled framework runtime class produced by
    /// `runtime_loader::go_units`. `file_name` is the basename the
    /// runtime loader chose for the class (e.g. `inflector.go`,
    /// `active_record_base.go`).
    TranspiledRuntime { file_name: &'a str },

    /// Application model LC. `lc_name` is the IR class name
    /// (e.g. `Article`); the emitted file basename is its
    /// `snake_case` form.
    Model { lc_name: &'a str },

    /// Application controller LC. `lc_name` is the IR class name.
    Controller { lc_name: &'a str },

    /// Synthesized stub for apps that don't ship their own
    /// `application_record.rb`.
    SynthApplicationRecord,

    /// Synthesized stub for apps that don't ship their own
    /// `application_controller.rb`.
    SynthApplicationController,

    /// Generated `route_helpers.go` (lowered from `config/routes.rb`).
    RouteHelpers,

    /// Generated `importmap.go` (lowered from `config/importmap.rb`).
    Importmap,

    /// View bundle for a resource. `resource_snake` is the snake-case
    /// resource name (e.g. `articles`) — the file is emitted as
    /// `views_<resource>.go`.
    ViewBundle { resource_snake: &'a str },

    /// `routes_table.go` — declarative route list consumed by the
    /// router glue.
    RoutesTable,

    /// `dispatch.go` — per-controller switch invoked by the router
    /// glue.
    Dispatch,

    /// `schema_sql.go` — DDL constant the boot path passes to
    /// `OpenProductionDB`.
    SchemaSql,

    /// `main.go` — the production binary entry. `package main`,
    /// emitted under `cmd/v2/`.
    Main,

    /// Test file: a `_test.go` re-emitted from the legacy go test
    /// pipeline, or one of the v2-only test helpers
    /// (`test_support_test.go`, `test_compat_test.go`).
    TestFile { file_name: &'a str },

    /// Sentinel file emitted when transpile fails — picked up by
    /// `go build` so the failure surfaces as a real build error.
    TranspileError,
}

/// Output destination for a file emitted by the go2 overlay.
pub(crate) struct OutputDest {
    /// Filesystem path (relative to the emit root) the file should
    /// land at.
    pub path: PathBuf,
    /// Go package name the file's `package X` header should declare.
    /// `cmd/v2/main.go` is `main`; everything else under `app/v2/`
    /// is `v2` (Phase 1 invariant). Phase 4 will return per-package
    /// names (`models`, `controllers`, `actioncontroller`, …);
    /// `rewrite_package` consumes this value.
    pub package: &'static str,
}

/// Resolve an `OutputKind` to its emitted path + package name.
///
/// Phase 1 invariant: every file lands under `app/v2/` in `package v2`,
/// except `Main` which lands at `cmd/v2/main.go` in `package main`.
/// Phase 4 (#19) replaces the body with per-package routing.
pub(crate) fn output_path(kind: OutputKind<'_>) -> OutputDest {
    use OutputKind::*;
    let (path, package): (String, &'static str) = match kind {
        HandWrittenRuntime { name } => (format!("app/v2/{name}"), "v2"),
        TranspiledRuntime { file_name } => (format!("app/v2/{file_name}"), "v2"),
        Model { lc_name } | Controller { lc_name } => (
            format!("app/v2/{}.go", crate::naming::snake_case(lc_name)),
            "v2",
        ),
        SynthApplicationRecord => ("app/v2/application_record.go".to_string(), "v2"),
        SynthApplicationController => ("app/v2/application_controller.go".to_string(), "v2"),
        RouteHelpers => ("app/v2/route_helpers.go".to_string(), "v2"),
        Importmap => ("app/v2/importmap.go".to_string(), "v2"),
        ViewBundle { resource_snake } => {
            (format!("app/v2/views_{resource_snake}.go"), "v2")
        }
        RoutesTable => ("app/v2/routes_table.go".to_string(), "v2"),
        Dispatch => ("app/v2/dispatch.go".to_string(), "v2"),
        SchemaSql => ("app/v2/schema_sql.go".to_string(), "v2"),
        Main => ("cmd/v2/main.go".to_string(), "main"),
        TestFile { file_name } => (format!("app/v2/{file_name}"), "v2"),
        TranspileError => ("app/v2/transpile_error.txt".to_string(), "v2"),
    };
    OutputDest {
        path: PathBuf::from(path),
        package,
    }
}

/// Return the Go package the named class is emitted into.
///
/// The argument is the canonical IR class name (Ruby-shape, e.g.
/// `"ActiveRecord::Base"` not the Go-emit-sanitized `"ActiveRecordBase"`).
/// This is the source-of-truth class-to-package map; cross-class
/// reference emit (#19 Phase 3) consults it to decide whether to
/// qualify a reference with its target package.
///
/// Phase 1 invariant: every class returns `"v2"`. The match arms below
/// pre-document the intended Phase 4 layout (one match arm per #19
/// destination package); the cutover is "flip the right-hand strings."
///
/// Arms are organized to mirror the layout table in GitHub issue #19.
#[allow(dead_code)] // consumed by Phase 4.2 emit-site rewrites
pub(crate) fn package_for_class(class_name: &str) -> &'static str {
    match class_name {
        // pkg/runtime/activerecord/ — `ActiveRecord::Base` and its
        // collaborators (errors, connection pool, the registry from
        // #20 Session 2).
        "ActiveRecord::Base"
        | "ActiveRecord::ConnectionPool"
        | "ActiveRecord::Registry"
        | "ActiveRecord::RecordNotFound"
        | "ActiveRecord::RecordInvalid" => "v2",

        // pkg/runtime/actioncontroller/ — controllers + flash/session.
        // (#19 spec groups Flash/Session under actioncontroller even
        // though Rails-side they're ActionDispatch.)
        "ActionController::Base"
        | "ActionDispatch::Flash"
        | "ActionDispatch::Session" => "v2",

        // pkg/runtime/actionview/ — view helpers, JsonBuilder.
        "ActionView::ViewHelpers" | "JsonBuilder" => "v2",

        // pkg/runtime/inflector/ — standalone (one file).
        "Inflector" => "v2",

        // internal/router/ — the router runtime + app-emitted routes
        // table / dispatch / route helpers / importmap.
        "ActionDispatch::Router"
        | "RouteHelpers"
        | "Importmap" => "v2",

        // internal/models/ — app-defined models. Detect via the
        // ApplicationRecord inheritance chain at emit time.
        // (`ApplicationRecord` itself + concrete models.)
        "ApplicationRecord" => "v2",

        // internal/controllers/ — app-defined controllers. Detect
        // via the `*Controller` suffix or ApplicationController
        // chain.
        "ApplicationController" => "v2",

        // Fallback: app-defined classes the static match above
        // doesn't enumerate. Today everything lives in `v2`; Phase 4
        // routes these by name pattern (`*Controller` →
        // controllers, `Views::*` → views/<resource>, `*Params` →
        // params, otherwise → models).
        _ => "v2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout_preserved() {
        assert_eq!(
            output_path(OutputKind::HandWrittenRuntime { name: "db.go" })
                .path
                .to_string_lossy(),
            "app/v2/db.go"
        );
        assert_eq!(
            output_path(OutputKind::Model { lc_name: "Article" })
                .path
                .to_string_lossy(),
            "app/v2/article.go"
        );
        assert_eq!(
            output_path(OutputKind::Model {
                lc_name: "ApplicationRecord"
            })
            .path
            .to_string_lossy(),
            "app/v2/application_record.go"
        );
        assert_eq!(
            output_path(OutputKind::ViewBundle {
                resource_snake: "articles"
            })
            .path
            .to_string_lossy(),
            "app/v2/views_articles.go"
        );
        assert_eq!(
            output_path(OutputKind::Main).path.to_string_lossy(),
            "cmd/v2/main.go"
        );
    }

    #[test]
    fn package_v2_for_overlay_main_for_binary() {
        assert_eq!(
            output_path(OutputKind::HandWrittenRuntime { name: "db.go" }).package,
            "v2"
        );
        assert_eq!(output_path(OutputKind::Main).package, "main");
        assert_eq!(
            output_path(OutputKind::RoutesTable).package,
            "v2"
        );
    }

    #[test]
    fn package_for_class_pins_flat_layout_invariant() {
        // Phase 1 invariant: every canonical class IR name resolves to
        // "v2". When Phase 4 flips the layout, this test fails as a
        // signal that the cutover landed (and a new test pins the
        // intended per-class destinations).
        for class in [
            "ActiveRecord::Base",
            "ActiveRecord::Registry",
            "ActiveRecord::RecordNotFound",
            "ActionController::Base",
            "ActionDispatch::Flash",
            "ActionDispatch::Session",
            "ActionDispatch::Router",
            "ActionView::ViewHelpers",
            "JsonBuilder",
            "Inflector",
            "RouteHelpers",
            "Importmap",
            "ApplicationRecord",
            "ApplicationController",
            "Article",
            "ArticlesController",
            "ArticleParams",
            "Views::Articles",
        ] {
            assert_eq!(
                package_for_class(class),
                "v2",
                "class {class} should resolve to v2 under flat layout"
            );
        }
    }
}
