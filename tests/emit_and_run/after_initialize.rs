//! `after_initialize` callbacks run at boot, in Rails' order, with their
//! bodies analyzed like the app's own code (`ingest::after_initialize`).
//!
//! The app is the one `topics/after-initialize-analyzed/oracle.rb` boots
//! with Rails 8.1.4 on CRuby 4.0.5; `ASSERTIONS` are its answers:
//! `entries=["application", "patched", "configure", "required"]`,
//! `greet="hello ann!"`, `original_greet="hello ann"`, `wave="wave"`.

use super::emit_and_run;

const APPLICATION: &str = r#"require "rails"
require "active_record/railtie"
module Probe
  class Application < Rails::Application
    config.load_defaults 8.1
    config.after_initialize do
      BootLog.record("application")
    end
  end
end
"#;

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
        .write("app/models/greeting.rb", "module Greeting\n  def greet(name)\n    \"hello \" + name\n  end\nend\n")
        .write("app/models/greeter.rb", "class Greeter\n  include Greeting\nend\n")
}

/// Six callbacks: the Application class body's, a reopen of a compiled
/// module with `alias_method`, one inside `Rails.application.configure`
/// that reads what the earlier ones recorded, one that leaves by `next`
/// before recording, one gated on `Rails::Server` (defined on neither lane,
/// as under Rails without `rails server`), and one that requires an app file
/// holding a reopen.
fn app() -> emit_and_run::Overlay {
    base()
        .write("config/application.rb", APPLICATION)
        .write("config/initializers/a_patch.rb", r#"Rails.application.config.after_initialize do
  Greeting.module_eval do
    alias_method :original_greet, :greet

    def greet(name)
      original_greet(name) + "!"
    end
  end
  BootLog.record("patched")
end
"#)
        .write("config/initializers/b_order.rb", r#"Rails.application.configure do
  config.after_initialize do
    next if BootLog.entries.empty?
    BootLog.record("configure")
  end
end

Rails.application.config.after_initialize do
  next unless BootLog.entries.empty?
  BootLog.record("skipped")
end

Rails.application.config.after_initialize do
  next unless defined?(Rails::Server)
  BootLog.record("server")
end
"#)
        .write("config/initializers/c_require.rb", r#"Rails.application.config.after_initialize do
  require Rails.root.join("lib/ext/greeter_patch").to_s
end
"#)
        .write("lib/ext/greeter_patch.rb", r#"Greeter.class_eval do
  def wave
    "wave"
  end
end
BootLog.record("required")
"#)
}

const ASSERTIONS: &str = r#"
entries = BootLog.entries
raise "after_initialize order: #{entries.inspect}" unless entries == ["application", "patched", "configure", "required"]
greeter = Greeter.new
raise "patched greet: #{greeter.greet("ann").inspect}" unless greeter.greet("ann") == "hello ann!"
raise "aliased original: #{greeter.original_greet("ann").inspect}" unless greeter.original_greet("ann") == "hello ann"
raise "required reopen: #{greeter.wave.inspect}" unless greeter.wave == "wave"
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

/// The body is analyzed like the app's own code: a call no runtime
/// implements stays an error, instead of a clean check over a boot that
/// raises. `I18n.backend` (a store the runtime's I18n does not have) and a
/// class method the model does not define.
#[test]
fn a_callback_call_with_no_runtime_behind_it_is_an_error() {
    let (_tree, errors) = base()
        .write("config/initializers/warm.rb", r#"Rails.application.config.after_initialize do
  I18n.backend.store_translations(:pl, {})
  Widget.warm_cache!
end
"#)
        .emit(roundhouse::project::BuildTarget::Ruby);
    for call in ["no known method `backend` on I18n", "no known method `warm_cache!` on Widget"] {
        assert!(errors.iter().any(|e| e.contains(call)), "no error `{call}`: {errors:#?}");
    }
}

/// A call on a value with no type (the registry's gradual `Rails.application`
/// and `Rails.cache`) is checked by nothing, so the callback is not carried
/// as supported: an error per unchecked chain, instead of a clean check over
/// a boot that may raise `NoMethodError`.
#[test]
fn a_callback_call_on_an_untyped_value_is_an_error() {
    let (_tree, errors) = base()
        .write("config/initializers/rotate.rb", r#"Rails.application.config.after_initialize do
  Rails.application.config.action_dispatch.cookies_rotations.rotate(:signed, "old")
  store = Rails.cache
  store.clear
end
"#)
        .emit(roundhouse::project::BuildTarget::Ruby);
    let unchecked: Vec<&String> = errors.iter().filter(|e| e.contains("after_initialize callback call")).collect();
    for call in ["`config` has no typed receiver", "`clear` has no typed receiver"] {
        assert!(unchecked.iter().any(|e| e.contains(call)), "no error `{call}`: {errors:#?}");
    }
    assert_eq!(unchecked.len(), 2, "one error per unchecked chain: {unchecked:#?}");
}

/// `Rails.root` is a Pathname in Rails and `Rails::AppPath` on both lanes,
/// typed so its calls are checked. The storage-directory callback an app
/// writes (`Rails.root.join("storage", dir).mkpath`) makes the directories
/// as Rails 8.1.4 does (`topics/after-initialize-analyzed/oracle_mkpath.rb`):
/// both exist after boot, `mkpath` answers the path, and a second call is a
/// no-op.
fn mkpath_app() -> emit_and_run::Overlay {
    base().write("config/initializers/storage_paths.rb", r#"Rails.application.config.after_initialize do
  %w[db files].each do |dir|
    Rails.root.join("tmp", "boot_paths", dir).mkpath
  end
end
"#)
}

const MKPATH_ASSERTIONS: &str = r#"
raise "boot_paths/db missing" unless Dir.exist?("tmp/boot_paths/db")
raise "boot_paths/files missing" unless Dir.exist?("tmp/boot_paths/files")
again = Rails.root.join("tmp", "boot_paths", "db")
raise "mkpath does not answer the path" unless again.mkpath.to_s == again.to_s
puts "Rails.root mkpath ran"
"#;

#[test]
fn a_rails_root_mkpath_callback_runs_on_ruby() {
    // Pathname#mkpath raises Errno::EEXIST through a file, as here.
    let through_a_file = r#"
File.write("tmp/boot_paths/plain", "")
begin
  Rails.root.join("tmp", "boot_paths", "plain", "x").mkpath
  raise "mkpath through a file did not raise"
rescue Errno::EEXIST
end
"#;
    let run = mkpath_app().run_ruby(&format!("{MKPATH_ASSERTIONS}{through_a_file}"));
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root mkpath ran"));
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_rails_root_mkpath_callback_runs_on_spinel() {
    mkpath_app().run_spinel(MKPATH_ASSERTIONS).assert_passes();
}

/// A target that does not boot through `boot.rb` cannot run a callback:
/// each is ledgered there. Crystal stands in for them.
#[test]
fn a_strict_target_ledgers_each_callback() {
    let (_tree, errors) = base()
        .write("config/initializers/log.rb", "Rails.application.config.after_initialize do\n  BootLog.record(\"x\")\nend\n")
        .emit(roundhouse::project::BuildTarget::Crystal);
    assert!(
        errors.iter().any(|e| e.contains("crystal has no boot step that runs an after_initialize callback")),
        "callback not ledgered on Crystal: {errors:#?}"
    );
}

fn ingest_error(files: &[(&str, &str)]) -> String {
    let mut all: Vec<(&str, &str)> = vec![("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")];
    all.extend_from_slice(files);
    let tree = all.iter().map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec())).collect();
    match roundhouse::ingest::ingest_app_from_tree(tree) {
        Ok(_) => panic!("ingest accepted {files:?}"),
        Err(err) => err.to_string(),
    }
}

/// A reopen of a module the compiled program does not define (a gem's)
/// would silently not apply: it is an ingest error, as is a `require` of
/// a file the program does not carry.
#[test]
fn a_reopen_of_an_uncompiled_module_and_a_foreign_require_are_errors() {
    let reopen = ingest_error(&[(
        "config/initializers/patch.rb",
        "Rails.application.config.after_initialize do\n  SomeGem::Finders.module_eval do\n    def find(id)\n      id\n    end\n  end\nend\n",
    )]);
    assert!(reopen.contains("reopens `SomeGem::Finders` with `module_eval`"), "{reopen}");
    let require = ingest_error(&[(
        "config/initializers/load.rb",
        "Rails.application.config.after_initialize do\n  require \"some_gem/patch\"\nend\n",
    )]);
    assert!(require.contains("names no app file the compiled program carries"), "{require}");
}
