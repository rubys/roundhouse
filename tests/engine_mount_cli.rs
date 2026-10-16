//! Literal source-backed isolated engines compose through the ordinary route
//! scope; Rack, dynamic and helper-bearing mount shapes remain explicit errors.

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

/// Strict emission composes an in-tree isolated engine on every route target.
#[test]
fn strict_transpile_composes_a_literal_isolated_engine_mount() {
    let fixture = Fixture::new("literal");
    let app = fixture.write_app(true);
    std::fs::write(
        app.join("vendor/catalog/lib/catalog/version.rb"),
        "module Catalog\n  VERSION = \"0.1.0\"\nend\n",
    )
    .unwrap();
    std::fs::write(
        app.join("vendor/catalog/lib/catalog.rb"),
        "require \"rails/engine\"\nrequire \"catalog/version\"\nrequire_relative \"catalog/engine\"\n",
    )
    .unwrap();
    for target in ["ruby", "spinel", "roda"] {
        let out = fixture.0.join(target);
        let result = transpile(&app, target, &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{target}: {stderr}");
        assert!(!stderr.contains("route mount"), "{target}: {stderr}");
        let route_file = if target == "roda" { "app.rb" } else { "config/routes.rb" };
        let routes = std::fs::read_to_string(out.join(route_file)).unwrap();
        let expected = if target == "roda" { "catalog" } else { "/catalog/products" };
        assert!(routes.contains(expected), "{target} route missing: {routes}");
    }
}

/// CLI smoke: one authentication guard fails strict emission; survey keeps
/// the unguarded sibling. Full guard coverage lives in `tests/ingest.rs`.
#[test]
fn authentication_route_guard_fail_closed_cli_smoke() {
    let fixture = Fixture::new("auth-route-guards");
    let app = fixture.write_app(false);
    std::fs::write(
        app.join("config/routes.rb"),
        "Rails.application.routes.draw do\n  authenticate :user do\n    get \"/account\", to: \"widgets#index\"\n  end\nend\n",
    )
    .unwrap();
    let strict_out = fixture.0.join("strict");
    let strict = transpile(&app, "spinel", &strict_out, &[]);
    let strict_stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(!strict.status.success(), "strict mode accepted authenticate: {strict_stderr}");
    assert!(
        strict_stderr.contains("unsupported routes DSL: `authenticate`"),
        "{strict_stderr}"
    );
    assert!(!strict_out.exists(), "strict mode wrote output for authenticate");

    std::fs::write(
        app.join("config/routes.rb"),
        "Rails.application.routes.draw do\n  authenticate :user do\n    get \"/account\", to: \"widgets#index\"\n  end\n  get \"/widgets\", to: \"widgets#index\"\nend\n",
    )
    .unwrap();
    let survey_out = fixture.0.join("survey");
    let surveyed = transpile(&app, "spinel", &survey_out, &["--survey"]);
    let survey_stderr = String::from_utf8_lossy(&surveyed.stderr);
    assert!(surveyed.status.success(), "survey should keep the sibling: {survey_stderr}");
    assert!(survey_stderr.contains("Survey: 1 ingest gap(s)"), "{survey_stderr}");
    assert!(
        survey_stderr.contains("unsupported routes DSL: `authenticate`"),
        "{survey_stderr}"
    );
    let routes = std::fs::read_to_string(survey_out.join("config/routes.rb")).unwrap();
    assert!(routes.contains("/widgets"), "unguarded sibling lost: {routes}");
    assert!(!routes.contains("/account"), "guarded route escaped: {routes}");
}

/// Nested literal engine namespaces can be loaded through their ancestor
/// modules without allowing declarations in those ancestors.
#[test]
fn strict_transpile_composes_a_nested_engine_namespace() {
    let fixture = Fixture::new("nested-owner");
    let app = fixture.write_app(true);
    std::fs::write(
        app.join("vendor/catalog/lib/catalog.rb"),
        "require \"rails/engine\"\nrequire_relative \"catalog/engine\"\n",
    )
    .unwrap();
    std::fs::write(
        app.join("vendor/catalog/lib/catalog/engine.rb"),
        "module Catalog\n  module Admin\n    class Engine < Rails::Engine\n      isolate_namespace Catalog::Admin\n    end\n  end\nend\n",
    )
    .unwrap();
    std::fs::remove_file(app.join("vendor/catalog/app/controllers/catalog/products_controller.rb"))
        .unwrap();
    std::fs::create_dir_all(app.join("vendor/catalog/app/controllers/catalog/admin"))
        .unwrap();
    std::fs::write(
        app.join("vendor/catalog/app/controllers/catalog/admin/products_controller.rb"),
        "module Catalog\n  module Admin\n    class ProductsController < ActionController::Base\n      def index\n        render plain: \"products\"\n      end\n    end\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(
        app.join("vendor/catalog/config/routes.rb"),
        "Catalog::Admin::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n",
    )
    .unwrap();
    std::fs::write(
        app.join("config/routes.rb"),
        "Rails.application.routes.draw do\n  mount Catalog::Admin::Engine, at: \"/catalog/admin\"\nend\n",
    )
    .unwrap();

    let out = fixture.0.join("spinel");
    let result = transpile(&app, "spinel", &out, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    let routes = std::fs::read_to_string(out.join("config/routes.rb")).unwrap();
    assert!(routes.contains("/catalog/admin/products"), "{routes}");
}

/// The normal diagnostic policy still rejects Rack mounts and preserves siblings.
#[test]
fn allow_unsupported_reports_a_rack_mount_and_keeps_host_routes() {
    let fixture = Fixture::new("allow");
    let app = fixture.write_app(false);
    std::fs::write(
        app.join("config/routes.rb"),
        "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n  mount ExternalRackApp, at: \"/catalog\"\nend\n",
    ).unwrap();
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

/// Only the exact literal isolated-engine form composes. Dynamic paths,
/// `as:` proxy names, nested mounts and repeated engine classes stay errors.
#[test]
fn complex_engine_mount_shapes_remain_explicit_errors() {
    let fixture = Fixture::new("complex");
    let app = fixture.write_app(false);
    for (label, routes) in [
        (
            "dynamic-path",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n  mount Catalog::Engine, at: ENV.fetch(\"MOUNT_PATH\")\nend\n",
        ),
        (
            "helper-alias",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n  mount Catalog::Engine, at: \"/catalog\", as: :catalog\nend\n",
        ),
        (
            "nested",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n  namespace :admin do\n    mount Catalog::Engine, at: \"/catalog\"\n  end\nend\n",
        ),
        (
            "twice",
            "Rails.application.routes.draw do\n  mount Catalog::Engine, at: \"/catalog\"\n  mount Catalog::Engine, at: \"/other\"\nend\n",
        ),
    ] {
        std::fs::write(app.join("config/routes.rb"), routes).unwrap();
        let out = fixture.0.join(format!("out-{label}"));
        let result = transpile(&app, "spinel", &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{label}: {stderr}");
        assert!(stderr.contains("error[unsupported]: route mount"), "{label}: {stderr}");
        assert!(!out.exists(), "strict mode wrote partial output for {label}");
    }
}

/// Protected engine routes are not made public by flattening their scope.
#[test]
fn authentication_guards_and_devise_scope_inside_an_engine_remain_errors() {
    let fixture = Fixture::new("devise");
    let app = fixture.write_app(true);
    for (label, routes, needle) in [
        (
            "authenticate",
            "Catalog::Engine.routes.draw do\n  authenticate :user do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
            "authentication guards in engine routes",
        ),
        (
            "authenticated",
            "Catalog::Engine.routes.draw do\n  authenticated :user do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
            "authentication guards in engine routes",
        ),
        (
            "unauthenticated",
            "Catalog::Engine.routes.draw do\n  unauthenticated :user do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
            "authentication guards in engine routes",
        ),
        (
            "devise_scope",
            "Catalog::Engine.routes.draw do\n  devise_scope :user do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
            "`devise_scope` in engine routes is not composed",
        ),
    ] {
        std::fs::write(app.join("vendor/catalog/config/routes.rb"), routes).unwrap();
        let out = fixture.0.join(format!("out-{label}"));
        let result = transpile(&app, "spinel", &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{label}: {stderr}");
        assert!(stderr.contains(needle), "{label}: {stderr}");
        assert!(!out.exists(), "{label}: wrote output");
    }
}

/// The shared host route parser flattens several request guards. An engine
/// mount cannot inherit that behavior because it would expose guarded routes.
#[test]
fn conditional_and_constrained_engine_routes_remain_explicit_errors() {
    let fixture = Fixture::new("route-guards");
    let app = fixture.write_app(true);
    for (label, routes) in [
        (
            "lambda-constraints-block",
            "Catalog::Engine.routes.draw do\n  constraints ->(request) { request.ssl? } do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "scope-constraints-option",
            "Catalog::Engine.routes.draw do\n  scope constraints: { subdomain: \"api\" } do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "route-constraints-option",
            "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\", constraints: { id: /\\d+/ }\nend\n",
        ),
        (
            "dynamic-root-target",
            "Catalog::Engine.routes.draw do\n  root to: redirect(root_path)\nend\n",
        ),
        (
            "dynamic-verb-target",
            "Catalog::Engine.routes.draw do\n  get \"/products\", to: redirect(products_path)\nend\n",
        ),
        (
            "resources-dsl",
            "Catalog::Engine.routes.draw do\n  resources :products, only: :index\nend\n",
        ),
        (
            "scope-wrapper",
            "Catalog::Engine.routes.draw do\n  scope path: \"/v1\" do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "conditional-routes",
            "Catalog::Engine.routes.draw do\n  if Rails.env.production?\n    get \"/products\", to: \"products#index\"\n  else\n    get \"/preview\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "unless-routes",
            "Catalog::Engine.routes.draw do\n  unless Rails.env.development?\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "repeated-route-draws",
            "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\nCatalog::Engine.routes.draw do\n  get \"/preview\", to: \"products#index\"\nend\n",
        ),
        (
            "top-level-route-code",
            "require \"catalog/custom_routes\"\nCatalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n",
        ),
        (
            "route-prepend",
            "Catalog::Engine.routes.draw do\n  routes.prepend do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "case-routes",
            "Catalog::Engine.routes.draw do\n  case Rails.env\n  when \"production\"\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "loop-routes",
            "Catalog::Engine.routes.draw do\n  2.times do\n    get \"/products\", to: \"products#index\"\n  end\nend\n",
        ),
        (
            "receiver-qualified-route",
            "Catalog::Engine.routes.draw do\n  self.get \"/products\", to: \"products#index\"\nend\n",
        ),
    ] {
        std::fs::write(app.join("vendor/catalog/config/routes.rb"), routes).unwrap();
        let out = fixture.0.join(format!("out-{label}"));
        let result = transpile(&app, "spinel", &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{label}: {stderr}");
        assert!(stderr.contains("error[unsupported]: route mount"), "{label}: {stderr}");
        assert!(!out.exists(), "strict mode wrote partial output for {label}");
    }
}

/// Engine naming, class initializers, and files under config/initializers can
/// change the mounted proxy or alter app middleware. The route-only slice
/// rejects those cases instead of claiming the mount was fully composed.
#[test]
fn customized_engine_boot_behavior_remains_an_explicit_error() {
    let fixture = Fixture::new("engine-boot");
    let app = fixture.write_app(true);
    for (label, engine_source, initializer_file) in [
        (
            "engine-name",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n    engine_name \"private_catalog\"\n  end\nend\n",
            None,
        ),
        (
            "namespace-mismatch",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Other\n  end\nend\n",
            None,
        ),
        (
            "engine-class-initializer",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n    initializer \"catalog.setup\" do\n      config.middleware.use Rack::Attack\n    end\n  end\nend\n",
            None,
        ),
        (
            "engine-file-side-effect",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\nRails.application.config.middleware.use Rack::Attack\n",
            None,
        ),
        (
            "engine-class-reopen",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\nmodule Catalog\n  class Engine\n    def self.engine_name\n      \"private_catalog\"\n    end\n  end\nend\n",
            None,
        ),
        (
            "initializers-directory",
            "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n",
            Some("Catalog::Engine.initializer(\"catalog.setup\") {}\n"),
        ),
    ] {
        std::fs::write(app.join("vendor/catalog/lib/catalog/engine.rb"), engine_source).unwrap();
        if let Some(source) = initializer_file {
            let path = app.join("vendor/catalog/config/initializers/setup.rb");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        } else {
            let path = app.join("vendor/catalog/config/initializers");
            if path.exists() {
                std::fs::remove_dir_all(path).unwrap();
            }
        }
        let out = fixture.0.join(format!("out-{label}"));
        let result = transpile(&app, "spinel", &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{label}: {stderr}");
        assert!(stderr.contains("error[unsupported]: route mount"), "{label}: {stderr}");
        assert!(!out.exists(), "strict mode wrote partial output for {label}");
    }
}

/// The gem entrypoint executes before mounting. It may load only checked
/// in-tree Ruby sources (and the Rails engine bootstrap); arbitrary top-level
/// calls or alias-based initializer registration keep the mount unsupported.
#[test]
fn engine_library_boot_side_effects_and_unchecked_requires_remain_errors() {
    let fixture = Fixture::new("engine-library-boot");
    let app = fixture.write_app(true);
    for (label, entrypoint) in [
        (
            "initializer",
            "require \"catalog/engine\"\nCatalog::Engine.initializer(\"catalog.middleware\") do |app|\n  app.config.middleware.use Rack::Attack\nend\n",
        ),
        (
            "initializer-alias",
            "require \"catalog/engine\"\nengine_class = Catalog::Engine\nengine_class.initializer(\"catalog.middleware\") { |app| app.config.middleware.use Rack::Attack }\n",
        ),
        (
            "unchecked-external-require",
            "require \"catalog/engine\"\nrequire \"catalog_bootstrap_with_hooks\"\n",
        ),
        (
            "global-kernel-reopen",
            "require \"catalog/engine\"\nmodule Kernel\n  def require(path)\n    false\n  end\nend\n",
        ),
        (
            "top-level-constant-alias",
            "require \"catalog/engine\"\nCATALOG_ENGINE = Catalog::Engine\n",
        ),
        (
            "load-hook-method",
            "require \"catalog/engine\"\nmodule Catalog\n  class PathReference\n    def self.inherited(child)\n      Catalog::Engine.initializer(\"catalog.middleware\") {}\n    end\n  end\nend\n",
        ),
        (
            "foreign-receiver-method",
            "require \"catalog/engine\"\nmodule Catalog\n  def Kernel.require(path)\n    false\n  end\nend\n",
        ),
    ] {
        std::fs::write(app.join("vendor/catalog/lib/catalog.rb"), entrypoint).unwrap();
        let out = fixture.0.join(format!("out-{label}"));
        let result = transpile(&app, "spinel", &out, &[]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{label}: {stderr}");
        assert!(stderr.contains("error[unsupported]: route mount"), "{label}: {stderr}");
        assert!(!out.exists(), "strict mode wrote partial output for {label}");
    }
}

/// A relative require cannot walk above the checked engine lib/ tree and
/// then lexically return to it, since the intermediate path may cross a link.
#[test]
fn engine_library_requires_reject_parent_directory_detours() {
    let fixture = Fixture::new("require-detour");
    let app = fixture.write_app(true);
    std::fs::write(
        app.join("vendor/catalog/lib/catalog.rb"),
        "require \"catalog/engine\"\nrequire_relative \"../lib/catalog/version\"\n",
    )
    .unwrap();
    std::fs::write(
        app.join("vendor/catalog/lib/catalog/version.rb"),
        "module Catalog\n  VERSION = \"0.1.0\"\nend\n",
    )
    .unwrap();
    std::fs::write(
        app.join("vendor/catalog/lib/catalog/engine.rb"),
        "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n",
    )
    .unwrap();

    let out = fixture.0.join("out");
    let result = transpile(&app, "spinel", &out, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "{stderr}");
    assert!(stderr.contains("error[unsupported]: route mount"), "{stderr}");
    assert!(!out.exists(), "strict mode wrote output for an out-of-tree require");
}

/// A link or out-of-root PATH entry cannot smuggle unrelated source into an
/// otherwise literal mount. Both cases retain the located mount error.
#[cfg(unix)]
#[test]
fn engine_route_sources_stay_inside_the_locked_path_root() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("source-root");
    let app = fixture.write_app(true);
    let outside = fixture.0.join("outside-routes.rb");
    std::fs::write(
        &outside,
        "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n",
    ).unwrap();
    std::fs::remove_file(app.join("vendor/catalog/config/routes.rb")).unwrap();
    symlink(&outside, app.join("vendor/catalog/config/routes.rb")).unwrap();
    let out = fixture.0.join("symlink-out");
    let result = transpile(&app, "spinel", &out, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "{stderr}");
    assert!(stderr.contains("error[unsupported]: route mount"), "{stderr}");
    assert!(!out.exists());

    let outside_root = fixture.0.join("shared");
    std::fs::create_dir_all(outside_root.join("lib/catalog")).unwrap();
    std::fs::create_dir_all(outside_root.join("app/controllers/catalog")).unwrap();
    std::fs::create_dir_all(outside_root.join("config")).unwrap();
    std::fs::write(
        outside_root.join("lib/catalog/engine.rb"),
        "module Catalog\n  class Engine < Rails::Engine\n    isolate_namespace Catalog\n  end\nend\n",
    ).unwrap();
    std::fs::write(
        outside_root.join("config/routes.rb"),
        "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\nend\n",
    ).unwrap();
    std::fs::write(
        app.join("Gemfile.lock"),
        "PATH\n  remote: ../shared\n  specs:\n    catalog (0.1.0)\n\nDEPENDENCIES\n  catalog!\n",
    ).unwrap();
    let out = fixture.0.join("outside-root-out");
    let result = transpile(&app, "spinel", &out, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success(), "{stderr}");
    assert!(stderr.contains("error[unsupported]: route mount"), "{stderr}");
    assert!(!out.exists());
}

/// Check reports the unsupported Rack mount beside an unrelated source error.
#[test]
fn check_reports_rack_mount_and_other_errors_together() {
    let fixture = Fixture::new("check");
    let app = fixture.write_app(false);
    std::fs::write(
        app.join("config/routes.rb"),
        "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\n  mount ExternalRackApp, at: \"/catalog\"\nend\n",
    ).unwrap();
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
