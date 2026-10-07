//! An unsupported engine mount must not disappear from a successful strict
//! transpile. Explicit emission overrides recover the supported sibling routes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Keep every CLI invocation isolated, including tests running in parallel.
struct Fixture(PathBuf);

impl Fixture {
    /// Reserve a unique parent for one test's app and emitted projects.
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "roundhouse-engine-mount-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Self(root)
    }

    /// A host route beside a real path-sourced engine with its own route.
    fn write_app(&self, mounted: bool) -> PathBuf {
        let app = self.0.join("app");
        for (path, source) in [
            ("app/controllers/widgets_controller.rb", "class WidgetsController < ActionController::Base\n  def index\n    render plain: \"widgets\"\n  end\nend\n"),
            ("db/schema.rb", "ActiveRecord::Schema[8.1].define do\nend\n"),
            ("Gemfile.lock", "PATH\n  remote: vendor/catalog\n  specs:\n    catalog (0.1.0)\n\nDEPENDENCIES\n  catalog!\n"),
            ("vendor/catalog/lib/catalog/engine.rb", "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n"),
            ("vendor/catalog/app/controllers/catalog/products_controller.rb", "module Catalog\n  class ProductsController < ActionController::Base\n    def index\n      render plain: \"products\"\n    end\n  end\nend\n"),
            ("vendor/catalog/config/routes.rb", "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n"),
        ] {
            let file = app.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, source).unwrap();
        }
        std::fs::create_dir_all(app.join("config")).unwrap();
        let mount = if mounted { "  mount Catalog::Engine, at: \"/catalog\"\n" } else { "" };
        std::fs::write(
            app.join("config/routes.rb"),
            format!("Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n{mount}end\n"),
        ).unwrap();
        app
    }
}

impl Drop for Fixture {
    /// Remove only the temporary tree owned by this test.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Exercise the public command instead of a library-only admission check.
fn transpile(app: &Path, target: &str, out: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target])
        .args(flags)
        .arg("--output").arg(out)
        .arg(app)
        .env_remove("ROUNDHOUSE_INGEST_SURVEY")
        .output()
        .expect("run roundhouse")
}

/// Strict emission rejects the error in either ingestion mode, before output.
#[test]
fn strict_transpile_refuses_an_engine_mount_without_writing_output() {
    let fixture = Fixture::new("strict");
    let app = fixture.write_app(true);
    for target in ["ruby", "spinel", "roda"] {
        for (label, flags) in [("strict", &[][..]), ("survey", &["--survey"][..])] {
            let out = fixture.0.join(format!("{target}-{label}"));
            let result = transpile(&app, target, &out, flags);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success(), "{target}/{label}: {stderr}");
            assert!(stderr.contains("config/routes.rb:3:3"), "{stderr}");
            assert!(stderr.contains("error[unsupported]: route mount"), "{stderr}");
            assert!(!out.exists(), "strict mode must not write incomplete output: {out:?}");
        }
    }
}

/// The normal diagnostic policy, not survey mode, controls emission recovery.
#[test]
fn allow_unsupported_reports_the_dropped_mount_and_keeps_host_routes() {
    let fixture = Fixture::new("allow");
    let app = fixture.write_app(true);
    for target in ["ruby", "spinel", "roda"] {
        for (label, flags) in [("allow", &["--allow-unsupported"][..]), ("survey-allow", &["--survey", "--allow-unsupported"][..])] {
            let out = fixture.0.join(format!("{target}-{label}"));
            let result = transpile(&app, target, &out, flags);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(result.status.success(), "{target}/{label}: {stderr}");
            assert!(stderr.contains("warning[unsupported]: route mount"), "{stderr}");
            assert!(stderr.contains("config/routes.rb:3:3"), "{stderr}");
            if label == "survey-allow" {
                assert!(stderr.contains("Survey: 1 ingest gap(s)"), "{stderr}");
            }
            let route_file = if target == "roda" { "app.rb" } else { "config/routes.rb" };
            let routes = std::fs::read_to_string(out.join(route_file)).unwrap();
            let expected = if target == "roda" { "r.get \"widgets\"" } else { "/widgets" };
            assert!(routes.contains(expected), "host route disappeared: {routes}");
            assert!(!routes.contains("catalog"), "override must not invent engine support: {routes}");
        }
    }
}

/// Check reports the route omission beside an unrelated source error.
#[test]
fn check_reports_mount_and_other_errors_together() {
    let fixture = Fixture::new("check");
    let app = fixture.write_app(true);
    std::fs::write(app.join("app/controllers/widgets_controller.rb"),
        "class WidgetsController < ActionController::Base\n  def index\n    render plain: 1.no_such_method\n  end\nend\n").unwrap();
    for mode in ["--strict", "--continue"] {
        let result = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
            .args(["check", mode]).arg(&app)
            .env_remove("ROUNDHOUSE_INGEST_SURVEY").output().unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("config/routes.rb:3:3"), "{stderr}");
        assert!(stderr.contains("error[unsupported]: route mount"), "{stderr}");
        assert!(stderr.contains("no_such_method"), "{stderr}");
        assert!(stderr.contains("app/controllers/widgets_controller.rb"), "{stderr}");
        assert!(!stderr.contains("ingest failed"), "{stderr}");
    }
}

/// Existing top-level cable mounts are provided by the shipped runtimes.
#[test]
fn builtin_action_cable_mounts_keep_the_fixed_runtime_endpoint() {
    let fixture = Fixture::new("cable");
    let app = fixture.write_app(false);
    // CRuby retains Cable only when an app has a live broadcast surface.
    std::fs::create_dir_all(app.join("app/models")).unwrap();
    std::fs::write(app.join("app/models/widget.rb"),
        "class Widget < ActiveRecord::Base\n  broadcasts_to ->(_widget) { 'widgets' }\nend\n").unwrap();
    std::fs::write(app.join("db/schema.rb"),
        "ActiveRecord::Schema[8.1].define do\n  create_table :widgets do |t|\n    t.string :name\n  end\nend\n").unwrap();
    for (label, mount) in [
        ("hashrocket", "mount ActionCable.server => '/cable'"),
        ("keyword", "mount ActionCable.server, at: '/cable'"),
    ] {
        std::fs::write(app.join("config/routes.rb"), format!(
            "Rails.application.routes.draw do\n  get '/widgets', to: 'widgets#index'\n  {mount}\nend\n"
        )).unwrap();
        for target in ["ruby", "spinel"] {
            let out = fixture.0.join(format!("{target}-{label}"));
            let result = transpile(&app, target, &out, &[]);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(result.status.success(), "{target}/{label}: {stderr}");
            assert!(!stderr.contains("route mount"), "{stderr}");
            let dispatch = if target == "ruby" { "config.ru" } else { "main.rb" };
            let source = std::fs::read_to_string(out.join(dispatch)).unwrap();
            assert!(source.contains("== \"/cable\""), "{target}: fixed cable dispatch absent");
        }
    }
}

/// Custom cable paths and nested mounts are not served by the fixed runtime.
#[test]
fn unimplemented_cable_mount_shapes_still_report_an_error() {
    let fixture = Fixture::new("cable-gap");
    let app = fixture.write_app(false);
    for (index, mount) in [
        "mount ActionCable.server, at: '/socket'",
        "mount ActionCable.server => '/cable', as: 'action_cable'",
        "namespace :admin do\n    mount ActionCable.server => '/cable'\n  end",
        "mount OtherCable.server => '/cable'",
    ].iter().enumerate() {
        std::fs::write(app.join("config/routes.rb"), format!(
            "Rails.application.routes.draw do\n  {mount}\nend\n"
        )).unwrap();
        let out = fixture.0.join(format!("out-{index}"));
        let result = transpile(&app, "ruby", &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{stderr}");
        assert!(stderr.contains("error[unsupported]: route mount"), "{stderr}");
        assert!(!out.exists());
    }
}

/// Runtime-provided ActiveStorage routes bypass unsupported host engine mounts.
#[test]
fn builtin_active_storage_routes_do_not_require_an_external_mount() {
    let fixture = Fixture::new("active-storage");
    let app = fixture.write_app(false);
    for target in ["ruby", "spinel"] {
        let out = fixture.0.join(target);
        let result = transpile(&app, target, &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{target}: {stderr}");
        assert!(!stderr.contains("route mount"), "{stderr}");
        let main = std::fs::read_to_string(out.join("main.rb")).unwrap();
        assert!(main.contains("RouteTable.table + ActiveStorage::Routes.table"), "{target}: built-in routes missing");
        assert!(out.join("runtime/active_storage_disk.rb").is_file());
    }
}
