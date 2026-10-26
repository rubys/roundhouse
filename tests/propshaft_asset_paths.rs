#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;

const APPLICATION_CSS: &str = "/* Intentionally empty: application styles live in app/frontend/styles (Vite + Tailwind). */\n";
const ICON: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0\"/></svg>\n";
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    propshaft (1.3.2)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  propshaft (= 1.3.2)\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("Gemfile.lock", LOCK)
        .write("config/application.rb", "module AssetProbe\n  class Application < Rails::Application\n    config.load_defaults 8.1\n    config.eager_load = false\n  end\nend\n")
        .write("config/initializers/assets.rb", "Rails.application.config.assets.version = \"1.0\"\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/pages_controller.rb", "class PagesController < ApplicationController\n  def index\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  root \"pages#index\"\nend\n")
        .write("app/views/pages/index.html.erb", "<%= stylesheet_link_tag \"application\" %>\n<%= image_tag \"icons/icon.svg\" %>\n<%= javascript_include_tag \"boot\" %>\n")
        .write("app/assets/stylesheets/application.css", APPLICATION_CSS)
        .write("app/assets/stylesheets/ui/theme.css", "body { background: url(\"../icons/icon.svg?v=1#shape\"); }\n")
        .write("app/assets/images/icons/icon.svg", ICON)
        .write("app/assets/javascripts/boot.js", "const icon = RAILS_ASSET_URL(\"icons/icon.svg\");\n")
        .write("app/assets/javascripts/bundle-abcdefgh.digested.js", "console.log('precompiled');\n")
        .write("app/assets/stylesheets/maps/mapped.css", "a {color:red}\n/*# sourceMappingURL=mapped.css.map */\n")
        .write("app/assets/stylesheets/maps/mapped.css.map", "{}\n")
        .write("app/assets/javascripts/mapped.js", "console.log('source');\n//# sourceMappingURL=mapped.js.map\n")
        .write("app/assets/javascripts/mapped.js.map", "{}\n")
}

#[test]
fn emitted_pages_and_asset_helpers_use_propshaft_paths() {
    app().run_ruby(r#"
out = StringIO.new
Main.run({"REQUEST_METHOD" => "GET", "PATH_INFO" => "/", "HTTP_HOST" => "example.test"}, StringIO.new, out)
html = out.string
raise html unless html.include?("200")
%w[/assets/application-55c8a0db.css /assets/icons/icon-d6065022.svg /assets/boot-cf177f03.js].each { |path| raise html unless html.include?(path) }
h = ActionView::ViewHelpers
raise unless h.asset_path("application.css?v=1#shape") == "/assets/application-55c8a0db.css?v=1#shape"
raise unless h.asset_path("application.css#shape?v=1") == "/assets/application-55c8a0db.css#shape?v=1"
raise unless h.asset_path("application.css", skip_pipeline: true) == "application.css"
raise unless h.image_path("icons/icon.svg", skip_pipeline: true) == "icons/icon.svg"
raise unless h.asset_path("/raw/icon.svg") == "/raw/icon.svg"
raise unless h.image_path("https://example.test/icon.svg") == "https://example.test/icon.svg"
raise unless h.image_url("icons/icon.svg") == "/assets/icons/icon-d6065022.svg"
raise unless h.javascript_path("boot") == "/assets/boot-cf177f03.js"
raise unless h.path_to_javascript("boot") == "/assets/boot-cf177f03.js"
puts "Emitted asset helpers and page paths match Propshaft"
"#).assert_passes();
}

#[test]
fn digested_static_files_have_propshaft_compiled_bytes() {
    let (emitted, app, errors) = app().emit_with_app(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    roundhouse::project::write_binary_assets(&app.binary_assets, &[], &emitted).unwrap();
    for (name, expected) in [
        ("application-55c8a0db.css", APPLICATION_CSS),
        ("icons/icon-d6065022.svg", ICON),
        (
            "ui/theme-ba5a2996.css",
            "body { background: url(\"/assets/icons/icon-d6065022.svg?v=1#shape\"); }\n",
        ),
        (
            "boot-cf177f03.js",
            "const icon = \"/assets/icons/icon-d6065022.svg\";\n",
        ),
        (
            "bundle-abcdefgh.digested.js",
            "console.log('precompiled');\n",
        ),
        (
            "maps/mapped-8b5a4e94.css",
            "a {color:red}\n/*# sourceMappingURL=/assets/maps/mapped-ee13c569.css.map */\n",
        ),
        ("maps/mapped-ee13c569.css.map", "{}\n"),
        (
            "mapped-3b39dabe.js",
            "console.log('source');\n//# sourceMappingURL=/assets/mapped-ee13c569.js.map\n",
        ),
        ("mapped-ee13c569.js.map", "{}\n"),
    ] {
        assert_eq!(
            std::fs::read(emitted.join("static/assets").join(name)).unwrap(),
            expected.as_bytes(),
            "{name}"
        );
    }
}

fn tree(extra: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    let mut files = [
        ("Gemfile.lock", LOCK),
        (
            "config/application.rb",
            "module AssetProbe\n class Application < Rails::Application\n end\nend\n",
        ),
        ("app/assets/stylesheets/application.css", APPLICATION_CSS),
    ]
    .into_iter()
    .map(|(name, text)| (PathBuf::from(name), text.as_bytes().to_vec()))
    .collect::<HashMap<_, _>>();
    for (name, text) in extra {
        files.insert(PathBuf::from(name), text.as_bytes().to_vec());
    }
    files
}

#[test]
fn version_is_consumed_and_initializers_override_environment_settings() {
    let files = tree(&[
        (
            "config/environments/production.rb",
            "Rails.application.configure do\n config.assets.version = \"production\"\nend\n",
        ),
        (
            "config/initializers/assets.rb",
            "Rails.application.config.assets.version = \"1.0\"\n",
        ),
    ]);
    let app = ingest_app_from_tree(files.clone()).unwrap();
    assert!(
        app.binary_assets
            .iter()
            .any(|(path, _)| path == "static/assets/application-55c8a0db.css")
    );
    let vfs = roundhouse::vfs::MapVfs::new(files);
    let ledger = roundhouse::ingest::initializer_ledger::ledger(
        &vfs,
        std::path::Path::new(""),
        &[],
        &Default::default(),
    );
    assert!(
        ledger
            .iter()
            .flat_map(|file| &file.verdicts)
            .any(|(_, verdict)| format!("{verdict:?}") == "Consumed(\"Propshaft asset version\")")
    );
}

#[test]
fn changing_asset_version_changes_the_served_filename() {
    let paths = ["1.0", "2.0"].map(|version| {
        let initializer = format!("Rails.application.config.assets.version = {version:?}\n");
        let app =
            ingest_app_from_tree(tree(&[("config/initializers/assets.rb", &initializer)])).unwrap();
        app.binary_assets
            .into_iter()
            .find(|(path, _)| path.starts_with("static/assets/application-"))
            .unwrap()
            .0
    });
    assert_ne!(paths[0], paths[1]);
}

#[test]
fn dynamic_or_environment_dependent_versions_are_refused() {
    for settings in [
        vec![(
            "config/initializers/assets.rb",
            "Rails.application.config.assets.version = ENV.fetch(\"ASSET_VERSION\")\n",
        )],
        vec![(
            "config/environments/production.rb",
            "Rails.application.configure do\n config.assets.version = \"production\"\nend\n",
        )],
    ] {
        let error = ingest_app_from_tree(tree(&settings))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Propshaft asset version") || error.contains("Propshaft asset versions"),
            "{error}"
        );
    }
}

#[test]
fn tailwind_app_stylesheet_alias_resolves_to_the_compiled_build() {
    emit_and_run::empty_app()
        .write("Gemfile.lock", LOCK)
        .write(
            "config/application.rb",
            "module AssetProbe\n  class Application < Rails::Application\n    config.load_defaults 8.1\n  end\nend\n",
        )
        .write("config/initializers/assets.rb", "Rails.application.config.assets.version = \"1.0\"\n")
        .write("app/assets/builds/tailwind.css", "body{color:red}\n")
        .run_ruby(
            r#"
app_path = Rails.application.propshaft_asset_path("app.css")
tailwind_path = Rails.application.propshaft_asset_path("tailwind.css")
raise "app.css alias missing" unless app_path == tailwind_path
raise "expected digested tailwind path" unless app_path.start_with?("/assets/tailwind-")
puts "tailwind :app alias resolves"
"#,
        )
        .assert_passes();
}

#[test]
fn apps_without_propshaft_keep_the_existing_asset_paths() {
    let mut files = tree(&[]);
    files.remove(&PathBuf::from("Gemfile.lock"));
    let app = ingest_app_from_tree(files).unwrap();
    assert!(
        !app.binary_assets
            .iter()
            .any(|(path, _)| path.starts_with("static/assets/"))
    );
    assert!(app.rails_application.as_ref().is_none_or(|class| {
        !class
            .methods
            .iter()
            .any(|method| method.name.as_str() == "propshaft_asset_path")
    }));
}
