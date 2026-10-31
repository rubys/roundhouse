//! PostgreSQL: concern included model callbacks, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::process::Command;

const RESOURCE_CALLBACKS: &str = r#"module ResourceCallbacks
  extend ActiveSupport::Concern

  included do
    extend ActiveModel::Callbacks

    define_model_callbacks :save, :destroy
    define_model_callbacks :reload, only: :after
  end
end
"#;

const RESOURCE_RESCUE: &str = r#"module ResourceRescue
  extend ActiveSupport::Concern

  included do
    around_save :guard
    around_destroy :guard
  end

  def guard
    @log << :guard_in
    yield
    @log << :guard_out
  end
end
"#;

const RESOURCE: &str = r#"class Resource
  include ResourceCallbacks
  include ResourceRescue

  attr_reader :log

  def initialize
    @log = []
  end

  def save
    run_callbacks(:save) { @log << :save; true }
  end

  def destroy
    run_callbacks(:destroy) { @log << :destroy }
  end

  def reload
    run_callbacks(:reload) { @log << :reload }
  end
end
"#;

const PROBE: &str = r##"class CallbacksProbe
  def self.lines
    out = []
    out << [Resource.respond_to?(:around_save), Resource.respond_to?(:before_destroy), Resource.respond_to?(:after_reload), Resource.respond_to?(:before_reload)].inspect
    out << Resource._save_callbacks.map { |c| [c.kind, c.filter] }.inspect
    Resource.after_reload { @log << :reloaded }
    r = Resource.new
    r.save
    r.destroy
    r.reload
    out << r.log.inspect
    out
  end
end
"##;

const EXPECTED: &[&str] = &[
    "[true, true, true, false]",
    "[[:around, :guard]]",
    "[:guard_in, :save, :guard_out, :guard_in, :destroy, :guard_out, :reload, :reloaded]",
];

/// In load order: each Concern before its includer.
const FILES: &[(&str, &str)] = &[
    ("app/services/concerns/resource_callbacks.rb", RESOURCE_CALLBACKS),
    ("app/services/concerns/resource_rescue.rb", RESOURCE_RESCUE),
    ("app/services/resource.rb", RESOURCE),
    ("app/models/callbacks_probe.rb", PROBE),
];

const LOCK_WITH_I18N: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\nend\n";

fn app() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .write("db/schema.rb", SCHEMA)
        // The ActiveModel::Validations port translates its messages through
        // I18n, which the PostgreSQL bootstrap requires in the lockfile.
        .write("Gemfile.lock", LOCK_WITH_I18N)
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_concern_included_model_callbacks\n")
}

fn native() -> Vec<String> {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let dir = std::env::temp_dir().join(format!("rh_concern_included_model_callbacks_{}", std::process::id()));
    for (path, text) in FILES {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let loads: String = FILES.iter().map(|(path, _)| format!("load {path:?}\n")).collect();
    let program = format!(
        "require {oracle:?}\nRailsOracle.activate(\"activesupport\", \"activemodel\")\nrequire \"active_support/all\"\nrequire \"active_model\"\n\
         {loads}puts CallbacksProbe.lines\n"
    );
    std::fs::write(dir.join("native.rb"), program).unwrap();
    let out = Command::new("ruby")
        .args(["-W0", "native.rb"])
        .current_dir(&dir)
        .env("GLIBC_TUNABLES", "glibc.rtld.optional_static_tls=262144")
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout.lines().map(str::to_string).collect()
}

/// The emitted includer runs the block's statements where Ruby does:
/// the `extend` and both `define_model_callbacks` right after the
/// include, then the later Concern's `around_*`, with nothing refused;
/// and the PostgreSQL CRuby tree carries the ActiveModel::Callbacks port
/// (runtime/active_model_validations.rb) the `extend` names.
#[test]
#[ignore = "fork spec: postgresql"]
fn included_extend_and_define_model_callbacks_are_carried_onto_a_plain_includer() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    // `check` names the PostgreSQL adapter as having no runtime of its
    // own in target-independent analysis; the CRuby tree carries one.
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let resource = std::fs::read_to_string(dir.join("app/models/resource.rb")).expect("resource.rb");
    assert!(!resource.contains("roundhouse:"), "{resource}");
    let body = "  include ResourceCallbacks\n  extend ActiveModel::Callbacks\n  define_model_callbacks :save, :destroy\n  \
                define_model_callbacks :reload, only: :after\n  include ResourceRescue\n  around_save :guard\n  around_destroy :guard\n";
    assert!(resource.contains(body), "{resource}");
    let validations = std::fs::read_to_string(dir.join("runtime/active_model_validations.rb")).expect("the port is carried");
    assert!(validations.contains("def define_model_callbacks"));
}

/// Native activemodel and the emitted tree print the same.
#[test]
#[ignore = "fork spec: postgresql"]
fn included_define_model_callbacks_run_on_a_plain_includer_as_activemodel() {
    let native = native();
    assert_eq!(native, EXPECTED, "native");
    let (dir, _) = app().emit(BuildTarget::Ruby);
    // Boot the tree without a connection: the probe never queries.
    let script = "require File.expand_path('main', Dir.pwd)\nputs CallbacksProbe.lines\n";
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args(["-W0", "-e", script])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let emitted: Vec<String> = stdout.lines().map(str::to_string).collect();
    assert_eq!(emitted, native, "emitted");
}
