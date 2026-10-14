//! `Rails.application.config.after_initialize do … end` in an initializer
//! runs once the app is booted, in initializer order, and a `next` in the
//! body leaves that callback only, as it leaves the block in Rails.

use super::emit_and_run;

fn base() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.string :label\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/boot_log.rb", r#"class BootLog
  ENTRIES = []

  def self.record(entry)
    ENTRIES << entry
  end

  def self.entries
    ENTRIES
  end
end
"#)
}

fn app() -> emit_and_run::Overlay {
    base()
        .write("config/initializers/a_first.rb", r#"Rails.application.config.after_initialize do
  BootLog.record("first")
end
"#)
        .write("config/initializers/b_second.rb", r#"Rails.application.config.after_initialize do
  next if BootLog.entries.empty?
  BootLog.record("second")
end

Rails.application.config.after_initialize do
  next unless BootLog.entries.empty?
  BootLog.record("skipped")
end
"#)
}

const ASSERTIONS: &str = r#"
raise "after_initialize callbacks: #{BootLog.entries.inspect}" unless BootLog.entries == ["first", "second"]
puts "after_initialize callbacks ran"
"#;

#[test]
fn after_initialize_callbacks_run_at_boot_on_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn after_initialize_callbacks_run_at_boot_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}

/// A class of the app's own named `AppInitializers` is not the generated
/// runner: with no after_initialize callbacks, boot does not call its
/// `run` (which here takes an argument).
#[test]
fn an_app_s_own_app_initializers_is_not_called_at_boot() {
    let (emitted, errors) = base()
        .write("lib/app_initializers.rb", "class AppInitializers\n  def self.run(phase)\n    phase\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let boot = std::fs::read_to_string(emitted.join("boot.rb")).expect("boot.rb");
assert!(!boot.contains("AppInitializers.run"), "{boot}");
// the app's own class is in the tree, so the name alone was there to go by
fn has_run(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        if p.is_dir() { has_run(&p) } else { std::fs::read_to_string(&p).is_ok_and(|t| t.contains("def self.run(phase)")) }
    })
}
assert!(has_run(&emitted), "AppInitializers is not in the emitted tree");
}
