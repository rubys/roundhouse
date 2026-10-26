use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;
use roundhouse::ty::Ty;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table :clocks do |t|\n    t.string :name\n  end\nend\n";
const CALLS: &[(&str, &str)] = &[
    ("iso", "Time.iso8601(\"2026-01-02T03:04:05+09:00\")"),
    ("xml", "Time.xmlschema(\"2026-01-02T03:04:05+09:00\")"),
    ("http", "Time.httpdate(\"Thu, 01 Jan 2026 18:04:05 GMT\")"),
    ("rfc", "Time.rfc2822(\"Fri, 02 Jan 2026 03:04:05 +0900\")"),
    ("rfc_alias", "Time.rfc822(\"Fri, 02 Jan 2026 03:04:05 +0900\")"),
    ("strptime", "Time.strptime(\"2026-01-02 03:04:05 +0900\", \"%Y-%m-%d %H:%M:%S %z\")"),
    ("parse", "Time.parse(\"2026-01-02T03:04:05+09:00\")"),
    ("now", "Time.now"),
    ("current", "Time.current"),
    ("at", "Time.at(0)"),
    ("utc", "Time.utc(2026, 1, 2)"),
    ("gm", "Time.gm(2026, 1, 2)"),
    ("local", "Time.local(2026, 1, 2)"),
    ("mktime", "Time.mktime(2026, 1, 2)"),
    ("new", "Time.new(2026, 1, 2)"),
];

fn model() -> String {
    let mut source = String::from("require \"time\"\nclass Clock < ApplicationRecord\n");
    for (name, call) in CALLS {
        source.push_str(&format!("  def self.from_{name}\n    {call}\n  end\n"));
    }
    source.push_str(r#"
  def self.time(value)
    Time.iso8601(value.to_s)
  rescue ArgumentError
    nil
  end
  def self.zone_time(value)
    Time.zone.parse(value.to_s)
  end
  def self.class_alias(value)
    clock = Time
    clock.iso8601(value.to_s)
  end
  def self.zone_local
    Time.zone.local(2026, 1, 2)
  end
  def self.zone_today
    Time.zone.today
  end
  def self.zone_now
    Time.zone.now
  end
  def self.zone_at
    Time.zone.at(0)
  end
  def self.zone_year(zone)
    Time.use_zone(zone) { Time.zone.now.year }
  end
  def self.formatted
    Time.at(0).iso8601
  end
  def self.xml_formatted
    Time.new(2026, 1, 2).xmlschema
  end
  def self.offset
    Time.iso8601("2026-01-02T03:04:05+09:00").utc_offset
  end
  def self.early_return
    return "earlier" if Time.now.year == 2026
    Time.iso8601("2026-01-02T03:04:05+09:00")
  end
  def self.mixed(value)
    value ? Time.iso8601("2026-01-02T03:04:05+09:00") : "missing"
  end
  def self.other_class
    Object.new
  end
end
"#);
    source
}

fn analyzed(source: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/clock.rb", source),
    ].into_iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn class_parsers_and_constructors_return_values_not_formatter_strings() {
    let app = analyzed(&model());
    for (name, _) in CALLS {
        let name = format!("from_{name}");
        let method = app.models[0].methods().find(|m| m.name.as_str() == name).unwrap();
        assert_eq!(method.body.ty, Some(Ty::Time), "{name}");
    }
    for (name, expected) in [("formatted", Ty::Str), ("xml_formatted", Ty::Str), ("offset", Ty::Int),
        ("class_alias", Ty::Time), ("zone_local", Ty::Time), ("zone_today", Ty::Time),
        ("zone_now", Ty::Time), ("zone_at", Ty::Time), ("zone_year", Ty::Int)] {
        let method = app.models[0].methods().find(|m| m.name.as_str() == name).unwrap();
        assert_eq!(method.body.ty, Some(expected), "{name}");
    }
}

#[test]
fn class_objects_do_not_borrow_instance_only_time_methods() {
    let app = analyzed(r#"class Clock < ApplicationRecord
  def self.bad_format = Time.strftime("%Y")
  def self.bad_offset = Time.utc_offset
  def self.bad_now = Time.now.now
  def self.parse_with_now = Time.parse("2026-01-02", Time.now)
end
"#);
    let mut failures: Vec<_> = diagnose(&app).into_iter().filter_map(|d| match d.kind {
        DiagnosticKind::SendDispatchFailed { method, .. } => Some(method.to_string()),
        _ => None,
    }).collect();
    failures.sort();
    assert_eq!(failures, ["now", "parse", "strftime", "utc_offset"]);
}

#[test]
fn emitted_rbs_keeps_parser_returns_and_rescued_nil() {
    let (tree, errors) = emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/clock.rb", &model())
        .emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let rbs = std::fs::read_to_string(tree.join("app/models/clock.rbs")).unwrap();
    assert!(rbs.contains("def self.time: (untyped value) -> Time?"), "{rbs}");
    assert!(rbs.contains("def self.zone_time: (untyped value) -> Time?"), "{rbs}");
    for name in ["iso", "xml", "http", "rfc", "rfc_alias", "strptime"] {
        assert!(rbs.contains(&format!("def self.from_{name}: () -> Time")), "{rbs}");
    }
    for name in ["early_return", "mixed", "other_class"] {
        let line = rbs.lines().find(|line| line.contains(&format!("def self.{name}:"))).unwrap();
        assert!(line.ends_with("-> untyped"), "{line}");
    }
}

#[test]
fn emitted_ruby_matches_cruby_parser_values_and_nil() {
    let source = model();
    let mut script = String::new();
    for (name, _) in CALLS {
        script.push_str(&format!("raise \"from_{name}\" unless Clock.from_{name}.is_a?(Time)\n"));
    }
    script.push_str(r#"
values = [Clock.from_iso.to_i, Clock.from_xml.to_i, Clock.from_http.to_i,
          Clock.from_rfc.to_i, Clock.from_rfc_alias.to_i, Clock.from_strptime.to_i,
          Clock.from_parse.to_i, Clock.offset, Clock.formatted,
          Clock.xml_formatted, Clock.time("invalid"), Clock.zone_time(""),
          Clock.zone_year("UTC")]
raise "zone_now" unless Clock.zone_now.year == Clock.zone_year("UTC")
puts values.inspect
"#);
    let oracle = emit_and_run::ruby().args(["-ractive_support/all", "-e", &format!(
        "class ApplicationRecord; end\n{source}\nTime.zone = 'UTC'\n{script}"
    )]).output().expect("run CRuby parser oracle with ActiveSupport");
    assert!(oracle.status.success(), "{}", String::from_utf8_lossy(&oracle.stderr));
    let run = emit_and_run::empty_app()
        .write("config/application.rb", "module TimeProbe\n  class Application < Rails::Application\n    config.time_zone = \"UTC\"\n  end\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/clock.rb", &source)
        .run_ruby(&script);
    run.assert_passes();
    assert_eq!(run.stdout, String::from_utf8(oracle.stdout).unwrap());
}
