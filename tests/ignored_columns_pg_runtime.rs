//! PostgreSQL: ignored columns, on the Ruby target against native Rails.
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
    legacy character varying DEFAULT 'db' NOT NULL,
    old_count integer DEFAULT 7 NOT NULL,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
CREATE TABLE gadgets (
    id bigserial NOT NULL,
    label character varying,
    legacy character varying DEFAULT 'gdb' NOT NULL,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_ignored_columns
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  self.ignored_columns = %w[legacy gone]
  self.ignored_columns += [:old_count]
end
"#;

const GADGET: &str = r#"class Gadget < ApplicationRecord
  self.ignored_columns = ::Widget.ignored_columns
end
"#;

const PROBE: &str = r##"class IgnoredColumnsProbe
  def self.lines
    out = []
    out << "list: #{Widget.ignored_columns.inspect} #{Gadget.ignored_columns.inspect}"
    w = Widget.create!(name: "a")
    out << "attribute: #{w.has_attribute?(:legacy)} #{w.has_attribute?(:name)} #{w.respond_to?(:legacy)} #{w.respond_to?(:old_count=)}"
    out << "insert: #{Widget.connection.select_value("SELECT legacy || '/' || old_count FROM widgets WHERE id = #{w.id}")}"
    Widget.connection.execute("UPDATE widgets SET legacy = 'kept', old_count = 9 WHERE id = #{w.id}")
    w.name = "b"
    w.save!
    out << "update: #{Widget.find(w.id).name} #{Widget.connection.select_value("SELECT legacy || '/' || old_count FROM widgets WHERE id = #{w.id}")}"
    g = Gadget.create!(label: "g")
    out << "gadget: #{Gadget.find(g.id).label} #{Gadget.connection.select_value("SELECT legacy FROM gadgets WHERE id = #{g.id}")}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    r#"list: ["legacy", "gone", "old_count"] ["legacy", "gone", "old_count"]"#,
    "attribute: false true false false",
    "insert: db/7",
    "update: b kept/9",
    "gadget: g gdb",
];

/// What native Rails requires, in order.
const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/widget", "app/models/gadget", "app/models/ignored_columns_probe"];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/gadget.rb", GADGET),
    ("app/models/ignored_columns_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries. The writes run as written at class load
/// (the runtime's `ignored_columns=`).
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: the ignored columns leave the compiled table and
/// the writes stay in the class body.
#[test]
#[ignore = "fork spec: postgresql"]
fn ignored_columns_leave_the_compiled_table() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let widget = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
    for expected in [
        r#"["legacy", "gone", "old_count"]"#,
        "  self.ignored_columns = %w[legacy gone]\n",
        // `+=` emits expanded.
        "  self.ignored_columns = self.ignored_columns + [:old_count]\n",
    ] {
        assert!(widget.contains(expected), "missing {expected:?} in:\n{widget}");
    }
    for absent in ["\"legacy\" =", "old_count ="] {
        assert!(!widget.contains(absent), "unexpected {absent:?} in:\n{widget}");
    }
    let gadget = std::fs::read_to_string(emitted.join("app/models/gadget.rb")).unwrap();
    assert!(gadget.contains("  self.ignored_columns = ::Widget.ignored_columns\n"), "{gadget}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn ignored_columns_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "ignored-columns",
        &FILES,
        &emitted,
        "rh_spec_ignored_columns",
        "IgnoredColumnsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}

/// A later write that disagrees with the compiled table raises, naming
/// the columns, in either direction: un-ignoring a column the compiled
/// table leaves out, or ignoring one it keeps. Rewriting the same list
/// passes.
#[test]
#[ignore = "fork spec: postgresql"]
fn an_ignored_columns_write_the_compiled_table_disagrees_with_raises() {
    // Class-level, so no adapter: require the emitted app and run.
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(
            r#"require File.expand_path("main", Dir.pwd)
def attempt
  yield
  puts "ok"
rescue NotImplementedError => e
  puts e.message
end
attempt { Widget.ignored_columns = %w[legacy] }
attempt { Widget.ignored_columns = [] }
attempt { Widget.ignored_columns += ["name"] }
attempt { Widget.ignored_columns = Widget.ignored_columns.dup }
puts Widget.ignored_columns.inspect
"#,
        )
        .current_dir(&*emitted)
        .output()
        .expect("spawn ruby");
    let (stdout, stderr) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "{stdout}\n{stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            r#"Widget.ignored_columns = ["legacy"]: un-ignores ["gone", "old_count"], which the compiled table leaves out"#,
            r#"Widget.ignored_columns = []: un-ignores ["legacy", "gone", "old_count"], which the compiled table leaves out"#,
            r#"Widget.ignored_columns = ["legacy", "gone", "old_count", "name"]: the compiled table leaves out only ["legacy", "gone", "old_count"]"#,
            "ok",
            r#"["legacy", "gone", "old_count"]"#,
        ],
        "{stderr}"
    );
}
