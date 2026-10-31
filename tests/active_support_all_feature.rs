//! PostgreSQL: active support all feature, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

const PROBE_GEM: &str = r#"require "active_support/all"
require "active_support/core_ext/hash"
require "active_support/core_ext/string/inflections"

module ProbeGem
  def self.report
    {
      blank: [nil.blank?, "".blank?, " \n".blank?, [].blank?, {}.present?, 0.blank?, "a".presence, "".presence, nil.present?],
      durations: [1.day.to_i, 2.hours.to_i, 3.weeks.to_i, 90.minutes.to_i, 1.day.class.name, (Time.at(0).utc + 1.day).to_i, 2.days == 172_800],
      hash: [{ a: { b: 1 } }.deep_merge(a: { c: 2 }), { a: 1 }.reverse_merge(a: 2, b: 3), { "a" => 1 }.symbolize_keys,
             { a: 1 }.with_indifferent_access["a"], { a: 1, b: 2 }.except(:b), { a: { b: 1 } }.deep_transform_values { |v| v + 1 },
             { a: 1, b: 2 }.slice(:a), { a: { "b" => 1 } }.deep_stringify_keys],
      inflections: ["active_record".camelize, "ActiveRecord".underscore, "post".pluralize, "posts".singularize,
                    "Admin::Post".demodulize, "x_y".humanize, "Post".constantize.name, "active_record".camelize(:lower)],
      strings: [" a \n b ".squish, "  x\n  y".strip_heredoc],
      try: ["a".try(:upcase), nil.try(:upcase)],
      to_xml: [{}.respond_to?(:to_xml), [].respond_to?(:to_xml), Hash.respond_to?(:from_xml), Hash.respond_to?(:from_trusted_xml)],
    }
  end
end

class Post; end
"#;

// On the tree only: what it does not port, by name.
const REFUSAL_GEM: &str = r#"require "active_support/all"

module ProbeGem
  def self.report
    refused = [-> { { a: 1 }.to_xml }, -> { [1].to_xml }, -> { Hash.from_xml("<a/>") }, -> { Hash.from_trusted_xml("<a/>") }].map do |call|
      call.call
      :answered
    rescue NotImplementedError => e
      e.message
    end
    named = [-> { 3.multiple_of?(3) }, -> { Time.now.plus_with_duration(1) }, -> { "x".multiple_of?(3) }].map do |call|
      call.call
      :answered
    rescue NoMethodError => e
      e.message.lines.map(&:chomp)
    end
    { refused: refused, named: named, rescued: (3.multiple_of?(3) rescue :rescued), respond_to: [3.respond_to?(:multiple_of?), {}.respond_to?(:to_xml)] }
  end
end
"#;

fn probe() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_active_support_all_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/probe.rb", "class Probe < ApplicationRecord\nend\n")
}

fn run(mode: &str, dir: &Path, gem: &Path) -> String {
    let out = emit_and_run::ruby()
        .arg(root().join("tests/framework_feature_requires.rb"))
        .arg("run")
        .arg(mode)
        .arg(dir)
        .arg(gem)
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{mode} failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

fn gem(scratch: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let lib = scratch.join(name).join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("probe_gem.rb"), text).unwrap();
    lib
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_gems_active_support_all_loads_the_ports_as_natively_rails_does() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let scratch = tree.parent().expect("scratch").to_path_buf();
    let lib = gem(&scratch, "probe_gem", PROBE_GEM);
    let native = run("native", &scratch.join("app"), &lib);
    let emitted = run("emitted", &tree, &lib);
    assert_eq!(emitted, native, "the tree's active_support/all answered the gem differently from Rails");
    // Not vacuous: the gem loaded past its requires and read each port.
    for expected in [
        "blank: [true, true, true, true, false, false, \"a\", nil, false]",
        "durations: [86400, 7200, 1814400, 5400, \"ActiveSupport::Duration\", 86400, true]",
        "hash: [{a: {b: 1, c: 2}}, {a: 1, b: 3}, {a: 1}, 1, {a: 1}, {a: {b: 2}}, {a: 1}, {\"a\" => {\"b\" => 1}}]",
        "inflections: [\"ActiveRecord\", \"active_record\", \"posts\", \"post\", \"Post\", \"X y\", \"Post\", \"activeRecord\"]",
        "to_xml: [true, true, true, true]",
    ] {
        assert!(native.contains(expected), "missing {expected:?}:\n{native}");
    }
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn what_active_support_all_does_not_port_refuses_by_name() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let scratch = tree.parent().expect("scratch").to_path_buf();
    let lib = gem(&scratch, "refusal_gem", REFUSAL_GEM);
    let out = run("emitted", &tree, &lib);
    let refused = |label: &str, feature: &str| {
        format!("\"roundhouse: error[framework_feature_not_ported]: Active Support's `{label}` ({feature}) is not ported; not run\"")
    };
    let conversions = "active_support/core_ext/hash/conversions";
    let line = format!(
        "refused: [{}, {}, {}, {}]\n",
        refused("Hash#to_xml", conversions),
        refused("Array#to_xml", "active_support/core_ext/array/conversions"),
        refused("Hash.from_xml", conversions),
        refused("Hash.from_trusted_xml", conversions)
    );
    assert!(out.contains(&line), "missing {line:?}:\n{out}");
    // The NameError stays one (rescued, `respond_to?` false), and names
    // the method only where Rails' all.rb defines it.
    let named = |message: &str, label: &str, feature: &str| {
        format!(
            "[{message:?}, \"roundhouse: error[framework_feature_not_ported]: Active Support's `{label}` ({feature}, which `require \\\"active_support/all\\\"` loads natively) is not ported on this tree\"]"
        )
    };
    let line = format!(
        "named: [{}, {}, [\"undefined method 'multiple_of?' for an instance of String\"]]\n",
        named("undefined method 'multiple_of?' for an instance of Integer", "Integer#multiple_of?", "active_support/core_ext/integer/multiple"),
        named("undefined method 'plus_with_duration' for an instance of Time", "Time#plus_with_duration", "active_support/core_ext/time/calculations"),
    );
    assert!(out.contains(&line), "missing {line:?}:\n{out}");
    assert!(out.contains("rescued: :rescued\n"), "{out}");
    assert!(out.contains("respond_to: [false, true]\n"), "{out}");
}

/// The tree's table of Rails' active_support/all methods is Rails' own.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn the_active_support_all_table_is_what_rails_all_defines() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let script = root().join("tests/active_support_all_methods.rb");
    let lines = |args: &[&std::ffi::OsStr]| -> String {
        let mut cmd = emit_and_run::ruby();
        cmd.arg(&script).args(args);
        let out = cmd.output().expect("ruby");
        assert!(out.status.success(), "{cmd:?} failed\n{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let native = lines(&["native".as_ref()]);
    let tree: &Path = &tree;
    let carried = lines(&["tree".as_ref(), tree.as_os_str()]);
    assert!(native.lines().count() > 400, "{native}");
    assert!(native.contains("active_support/core_ext/integer/time Integer#months\n"), "{native}");
    assert_eq!(carried, native, "project.rs::ACTIVE_SUPPORT_ALL_METHODS is not Rails' active_support/all");
}
