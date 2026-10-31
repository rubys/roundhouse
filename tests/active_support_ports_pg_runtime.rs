//! PostgreSQL: active support ports, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_active_support_ports
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const PROBE: &str = r##"require "fileutils"

class ActiveSupportPortsProbe
  def self.lines
    out = []
    root = File.join(ENV.fetch("TMPDIR", "/tmp"), "rh-as-ports-#{Process.pid}")
    FileUtils.rm_rf(root)
    store = ActiveSupport::Cache::FileStore.new(File.join(root, "sessions"))
    out << "write: #{store.write("session:abc", "a=1&b=2", expires_in: 600)}"
    out << "read: #{store.read("session:abc").inspect}"
    out << "fetch: #{store.fetch("card:k1") { "4242" }.inspect} #{store.read("card:k1").inspect}"
    out << "unless_exist: #{store.write("session:abc", "x", unless_exist: true)} #{store.read("session:abc").inspect}"
    out << "delete: #{store.delete("card:k1")} #{store.exist?("card:k1")}"
    out << "increment: #{store.increment("n")} #{store.increment("n", 4)}"
    out << "files: #{Dir.glob("**/*", base: store.cache_path).count { |p| File.file?(File.join(store.cache_path, p)) }}"
    calls = []
    css = File.join(root, "css")
    sheet = File.join(css, "a.css")
    FileUtils.mkdir_p(css)
    File.write(sheet, "x")
    File.utime(Time.at(1_600_000_000), Time.at(1_600_000_000), sheet)
    checker = ActiveSupport::FileUpdateChecker.new([], { css => ["css"] }) { calls << :ran }
    out << "checker: #{checker.updated?} #{checker.execute_if_updated}"
    File.utime(Time.at(1_600_000_100), Time.at(1_600_000_100), sheet)
    out << "changed: #{checker.updated?} #{checker.execute_if_updated} #{calls.inspect}"
    ms = ActiveSupport::Benchmark.realtime(:float_millisecond) { sleep 0.01 }
    out << "benchmark: #{ms.is_a?(Float)} #{ms >= 10}"
    store.clear
    FileUtils.rm_rf(root)
    out << "widgets: #{Widget.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    "write: true",
    r#"read: "a=1&b=2""#,
    r#"fetch: "4242" "4242""#,
    r#"unless_exist: false "a=1&b=2""#,
    "delete: true false",
    "increment: 1 5",
    "files: 2",
    "checker: false false",
    "changed: true true [:ran]",
    "benchmark: true true",
    "widgets: 0",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
    ("app/models/active_support_ports_probe.rb", PROBE),
];

fn overlay(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn active_support_ports_answer_as_natively_on_postgresql() {
    let (emitted, errors) = overlay(&FILES).emit(roundhouse::project::BuildTarget::Ruby);
    for port in ["active_support_file_store", "active_support_file_update_checker", "active_support_benchmark"] {
        assert!(emitted.join(format!("runtime/{port}.rb")).exists(), "{port} not carried");
    }
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-support-ports",
        &FILES,
        &emitted,
        "rh_spec_active_support_ports",
        "ActiveSupportPortsProbe",
        &["app/models/application_record", "app/models/widget", "app/models/active_support_ports_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

const CURRENT_ATTRIBUTES: &str = r##"class CurrentAttributesReader
  def self.bases
    [ActiveRecord::Base, ActiveSupport::CurrentAttributes]
  end

  def self.default?(value)
    value.equal?(ActiveSupport::CurrentAttributes::NOT_SET)
  end
end
"##;

#[test]
#[ignore = "fork spec: postgresql"]
fn ports_type_and_current_attributes_is_refused_by_name() {
    let mut files = FILES.to_vec();
    files.push(("app/models/current_attributes_reader.rb", CURRENT_ATTRIBUTES));
    let (emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    let errors: Vec<&String> = real_errors(&errors);
    let ports: Vec<_> = errors
        .iter()
        .filter(|e| e.contains("FileStore") || e.contains("FileUpdateChecker") || e.contains("Benchmark") || e.contains("Probe"))
        .collect();
    assert!(ports.is_empty(), "{ports:#?}");
    let reason = "Rails' CurrentAttributes base class is not carried";
    for name in ["ActiveSupport::CurrentAttributes", "ActiveSupport::CurrentAttributes::NOT_SET"] {
        let expected = format!("constant not supported (all targets): {name} — {reason}");
        assert!(errors.iter().any(|e| e.contains(&expected)), "no {expected:?} in:\n{errors:#?}");
    }
    let refusals = errors.iter().filter(|e| e.contains("constant not supported")).count();
    assert_eq!(refusals, 2, "{errors:#?}");
    // Loaded where the gem façades load, after the cache port.
    let facades = std::fs::read_to_string(emitted.join("runtime/gem_facades.rb")).expect("gem_facades.rb");
    let at = |name: &str| facades.find(&format!("require_relative \"{name}\"")).unwrap_or_else(|| panic!("no {name}:\n{facades}"));
    assert!(at("active_support_cache") < at("active_support_file_store"), "{facades}");
    at("active_support_file_update_checker");
    at("active_support_benchmark");
}

/// An app that names none of them carries none of them.
#[test]
fn ports_ride_only_where_named() {
    let files: Vec<_> = FILES.iter().copied().filter(|(path, _)| !path.ends_with("_probe.rb")).collect();
    let (emitted, _errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    for port in ["active_support_file_store", "active_support_file_update_checker", "active_support_benchmark"] {
        assert!(!emitted.join(format!("runtime/{port}.rb")).exists(), "{port} carried unnamed");
    }
}
