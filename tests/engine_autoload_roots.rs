//! PostgreSQL: engine autoload roots, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ENGINE: &str = r#"require "rails"

module Widgets
  class Engine < Rails::Engine
    paths.add("app/liquid/drops", eager_load: true)
    paths.add("app/liquid/lib", load_path: true)
  end
end
"#;

const DROP: &str = r#"puts "loading drop"

class WidgetDrop
  def self.name_for(id)
    "widget-#{id}"
  end
end

WidgetDrop.singleton_class.send(:define_method, :version) { 3 }
"#;

const PATH_GEMS: &str = "PATH\n  remote: engines/widgets\n  specs:\n    widgets (0.1.0)\n\nDEPENDENCIES\n  widgets!\n";

fn fixture() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_engine_autoload_roots\n")
        .write("Gemfile.lock", PATH_GEMS)
        .write("engines/widgets/lib/widgets/engine.rb", ENGINE)
        .write("engines/widgets/app/liquid/drops/widget_drop.rb", DROP)
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_engines_eager_load_path_is_an_autoload_root_for_its_files_constants() {
    let (emitted, errors) = fixture().emit(BuildTarget::Ruby);
    let own: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(own.is_empty(), "{own:?}");
    let models = std::fs::read_to_string(emitted.join("app/models.rb")).expect("app/models.rb");
    assert!(
        models.contains("[:WidgetDrop, \"models/roundhouse_files/engines/widgets/app/liquid/drops/widget_drop\", false]")
            || models.contains("[:WidgetDrop, \"models/widget_drop\", false]"),
        "WidgetDrop is a top-level autoload, not nested under Drops:\n{}",
        models.lines().filter(|l| l.contains("WidgetDrop") || l.contains("UNREACHED")).collect::<Vec<_>>().join("\n")
    );
    assert!(!models.contains("[:Drops, "), "no `Drops` namespace under the app/liquid root");
}

// The lock names the `liquid` gem, so `app/liquid/lib` reads as `<gem>/lib`.
const LIQUID_LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    liquid (5.5.0)\n\nPATH\n  remote: engines/widgets\n  specs:\n    widgets (0.1.0)\n\nDEPENDENCIES\n  liquid\n  widgets!\n";

const LIB_ENGINE: &str = r#"require "rails"

module Widgets
  class Engine < Rails::Engine
    paths.add("app/liquid/lib", eager_load: true)
  end
end
"#;

const SECTION_TEMPLATE: &str = r#"class SectionTemplate
  def self.parse(text)
    "parsed #{text}"
  end
end
"#;

const VISITOR: &str = r#"module StaticAnalysis
  module ParseTreeVisitor
    def self.visit(text)
      text.size
    end
  end
end
"#;

const ASSET: &str = r#"class ThemeAsset
  def parsed(text)
    SectionTemplate.parse(text)
  end

  def analysed(text)
    StaticAnalysis::ParseTreeVisitor.visit(text)
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn an_engines_lib_named_eager_load_root_autoloads_its_constants() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_engine_autoload_roots\n")
        .write("Gemfile.lock", LIQUID_LOCK)
        .write("engines/widgets/lib/widgets/engine.rb", LIB_ENGINE)
        .write("engines/widgets/app/liquid/lib/section_template.rb", SECTION_TEMPLATE)
        .write("engines/widgets/app/liquid/lib/static_analysis/parse_tree_visitor.rb", VISITOR)
        .write("app/models/theme_asset.rb", ASSET)
        .emit(BuildTarget::Ruby);
    let own: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(own.is_empty(), "{own:?}");
    let models = std::fs::read_to_string(emitted.join("app/models.rb")).expect("app/models.rb");
    assert!(
        models.contains("[:SectionTemplate, \"models/roundhouse_files/engines/widgets/app/liquid/lib/section_template\", false]")
            || models.contains("[:SectionTemplate, \"models/section_template\", false]"),
        "SectionTemplate is a top-level autoload:\n{}",
        models.lines().filter(|l| l.contains("SectionTemplate") || l.contains("UNREACHED")).collect::<Vec<_>>().join("\n")
    );
    // The directory under the root is a top-level namespace too
    // (`StaticAnalysis::ParseTreeVisitor`, read in a method body).
    assert!(
        models.contains("[:StaticAnalysis, "),
        "StaticAnalysis is a top-level namespace:\n{}",
        models.lines().filter(|l| l.contains("StaticAnalysis") || l.contains("ParseTreeVisitor")).collect::<Vec<_>>().join("\n")
    );
}
