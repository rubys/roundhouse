use super::emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.integer :number\n    t.integer :phase\n    t.string :label\n  end\nend\n")
        .write("app/models/widget.rb", &format!("class Widget < ApplicationRecord\n{MODEL_BODY}end\n"))
}

const MODEL_BODY: &str = r#"  enum :phase, { draft: 0, done: 1 }
  def self.lookup(conditions)
    find_by(conditions)
  end
"#;

const ASSERTIONS: &str = r#"
Db.exec("INSERT INTO widgets (id, number, label) VALUES (1, 7, 'present'), (2, NULL, 'unset')")
record = Widget.lookup(number: nil)
raise "model Hash finder misses NULL" unless record && record.id == 2
record = Widget.find_by(number: nil)
raise "class finder misses NULL" unless record && record.id == 2
record = Widget.lookup(number: [7, 9])
raise "model Hash finder misses IN values" unless record && record.id == 1
record = Widget.lookup({"number" => nil})
raise "string Hash keys miss NULL" unless record && record.id == 2
raise "empty IN matches rows" if Widget.lookup(number: [])
record = Widget.lookup(number: "7")
raise "numeric string equality regresses" unless record && record.id == 1
record = Widget.lookup({})
raise "empty Hash misses rows" unless record
record = Widget.find_by(number: 7)
raise "scalar finder regresses" unless record && record.id == 1
raise "compound conditions ignored" if Widget.lookup(number: nil, label: 'present')
raise "missing row returned" if Widget.lookup(number: [9, 10])
record = Widget.find_by!(number: nil)
raise "bang finder misses NULL" unless record.id == 2
begin
  Widget.find_by!(number: 99)
  raise "missing bang finder does not raise"
rescue ActiveRecord::RecordNotFound
end
puts "Model Hash finder predicates passed"
"#;

#[test]
fn model_hash_finder_predicates_run_on_ruby() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn model_hash_finder_predicates_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    app().run_spinel(&script).assert_passes();
}

// The same program runs on Rails and the emitted runtime. Include a
// real zero and NULL so invalid text cannot accidentally match either.
const STRING_QUERIES: &str = r##"
Widget.connection.execute("INSERT INTO widgets (id, number, phase, label) VALUES (0, 0, 0, 'zero'), (1, 7, 1, '7suffix'), (2, NULL, NULL, 'not-an-export')")
def equal_ids(expected, relation)
  actual = relation.order(:id).pluck(:id)
  raise "#{relation.to_sql}: #{actual.inspect}, expected #{expected.inspect}" unless actual == expected
end
bad = ["not-an-export", "", " \t", "+x", "_7"]
bad.each do |value|
  raise "invalid hash finder" if Widget.lookup(number: value)
  equal_ids([], Widget.where(number: value))
  equal_ids([], Widget.where.not(number: value))
  equal_ids([], Widget.where(number: [value]))
  equal_ids([2], Widget.where(number: [nil, value]))
  equal_ids([], Widget.where.not(number: [nil, value]))
  raise "invalid exists" if Widget.all.exists?(value)
  begin
    Widget.find(value)
    raise "invalid find returned a record"
  rescue ActiveRecord::RecordNotFound
  end
end
["7suffix", " \t+0007tail", "7__0", "7.9"].each do |value|
  raise "numeric-prefix hash finder" unless Widget.lookup(number: value).id == 1
  computed = {"number" => value.dup}
  raise "computed Hash finder" unless Widget.send(:find_by, computed).id == 1
  equal_ids([1], Widget.where(number: value))
  equal_ids([0], Widget.where.not(number: value))
  equal_ids([1], Widget.where({"number" => value}))
  equal_ids([1], Widget.send(:where, computed))
  equal_ids([1], Widget.where({"widgets.number" => value}))
  equal_ids([1], Widget.where(widgets: {number: value}))
  equal_ids([1, 2], Widget.where(number: [nil, value]))
  equal_ids([1], Widget.where(number: [7..8, value]))
end
equal_ids([0], Widget.where(number: "0junk"))
equal_ids([], Widget.where(number: "7_0"))
equal_ids([2], Widget.where(number: nil))
equal_ids([0, 1], Widget.where.not(number: nil))
equal_ids([1], Widget.where(label: "7suffix"))
equal_ids([2], Widget.where(label: "not-an-export"))
equal_ids([], Widget.where("number = ?", "7suffix"))
# Enum query attributes use a subtype: invalid text becomes IS NULL,
# so its negation keeps non-null rows. Integer storage alone is no proof.
equal_ids([0, 1], Widget.where.not(phase: "bogus"))
huge = "1" + "0" * 80
[huge, "-" + huge].each do |value|
  equal_ids([], Widget.where(number: value))
  equal_ids([0, 1, 2], Widget.where.not(number: value))
  equal_ids([], Widget.where(number: [value]))
  equal_ids([0, 1, 2], Widget.where.not(number: [value]))
  equal_ids([1], Widget.where(number: [value, "7suffix"]))
end
puts "integer query strings match"
"##;

fn assert_query_parity(app: emit_and_run::Overlay, model_body: &str, queries: &str) {
    // Keep the emitted regression active even when the optional oracle
    // is unavailable on a developer's machine.
    let emitted = app.run_ruby(queries);
    emitted.assert_passes();
    let oracle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let native = format!(r#"
require {oracle:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  warn "skipping native control: #{{RailsOracle.label}} or sqlite3 unavailable"
  exit 77
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:widgets) {{ |t| t.integer :number; t.integer :phase; t.string :label }}
end
class Widget < ActiveRecord::Base
{model_body}end
{queries}
"#);
    let native = emit_and_run::ruby().args(["-e", &native]).output().expect("native Rails control");
    if native.status.code() == Some(77) {
        eprintln!("{}", String::from_utf8_lossy(&native.stderr));
        return;
    }
    assert!(native.status.success(), "{}\n{}", String::from_utf8_lossy(&native.stdout), String::from_utf8_lossy(&native.stderr));
    assert_eq!(emitted.stdout, String::from_utf8_lossy(&native.stdout));
}

#[test]
fn integer_query_strings_match_native_rails() {
    assert_query_parity(app(), MODEL_BODY, STRING_QUERIES);
}

// Original scalar cardinality matters under negation: a singleton
// overflow is false, while a multi-value list that casts away is NULL.
const ARRAY_QUERIES: &str = r##"
Widget.connection.execute("INSERT INTO widgets (id, number) VALUES (0, 0), (1, 1), (2, NULL)")
huge = "9223372036854775808"
other_huge = "9223372036854775809"
cases = [
  [["1", "x"], [1], [0, 2], [1], [0]],
  [[1, "x"], [1], [0, 2], [1], [0]],
  [[1, "x", nil], [1], [0, 2], [1, 2], [0]],
  [["x", "y"], [], [], [], []],
  [[huge], [], [0, 1, 2], [], [0, 1, 2]],
  [[huge, nil], [], [0, 1, 2], [2], [0, 1]],
  [[huge, other_huge], [], [], [], []],
  [[huge, other_huge, nil], [], [], [2], []],
  [["1", "x", 0..0], [0, 1], [2], [0, 1], []],
  [[1, "x", nil, 0..0], [0, 1], [2], [0, 1, 2], []],
  [[huge, 0..0], [0], [1, 2], [0], [1]],
  [[huge, other_huge, 0..0], [0], [], [0], []],
  [["x", 0..0], [0], [], [0], []],
  [["x", "y", nil, 0..0], [0], [], [0, 2], []],
  [[nil, 0..0], [0], [1, 2], [0, 2], [1]],
  [[], [], [0, 1, 2], [], [0, 1, 2]]
]
cases.each do |values, id_positive, id_negative, number_positive, number_negative|
  [[:id, id_positive, id_negative], [:number, number_positive, number_negative]].each do |column, expected_positive, expected_negative|
    conditions = {column.to_s => values.dup}
    positive = Widget.where(conditions).order(:id).pluck(:id)
    negative = Widget.where.not(conditions).order(:id).pluck(:id)
    actual = [positive, negative]
    expected = [expected_positive, expected_negative]
    raise "#{column} #{values.inspect}: #{actual.inspect}, expected #{expected.inspect}" unless actual == expected
    puts "#{column} #{values.inspect}: #{actual.inspect}"
  end
end
"##;

#[test]
fn integer_query_mixed_arrays_match_native_rails() {
    assert_query_parity(app(), MODEL_BODY, ARRAY_QUERIES);
}

const NORMALIZED_BODY: &str = r#"  normalizes :number, with: ->(n) { n == 0 ? nil : n }
  def self.excluding(conditions)
    where.not(conditions)
  end
"#;

const NORMALIZED_QUERIES: &str = r##"
Widget.connection.execute("INSERT INTO widgets (id, number, phase) VALUES (0, 0, 0), (1, 1, 1), (2, NULL, NULL)")
conditions = {"number" => ["a", "b", "c"].join}
ids = Widget.excluding(conditions).order(:id).pluck(:id)
raise "normalized query: #{ids.inspect}" unless ids == [0, 1]
ids = Widget.where(phase: "1suffix").order(:id).pluck(:id)
raise "plain integer excluded with normalized column" unless ids == [1]
puts "normalized integer query strings match"
"##;

#[test]
fn normalized_integer_queries_match_native_rails() {
    let app = app().write("app/models/widget.rb", &format!("class Widget < ApplicationRecord\n{NORMALIZED_BODY}end\n"));
    assert_query_parity(app, NORMALIZED_BODY, NORMALIZED_QUERIES);
}

#[test]
fn query_cast_evidence_excludes_decorated_integer_columns() {
    let (emitted, errors) = app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :widgets do |t|\n    t.integer :number\n    t.integer :phase\n    t.integer :declared\n    t.integer :normalized\n    t.string :label\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  enum :phase, { draft: 0, done: 1 }\n  attribute :declared, :integer\n  normalizes :normalized, with: ->(n) { n == 0 ? nil : n }\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
    let columns = source.split("def self.schema_integer_query_columns").nth(1).expect("query column evidence");
    let columns = columns.split("\n  end").next().unwrap();
    for name in [":id", ":number"] {
        assert!(columns.contains(name), "{columns}");
    }
    for name in [":phase", ":declared", ":normalized", ":label"] {
        assert!(!columns.contains(name), "{columns}");
    }
}

// Hash values are serialized before the bind marker is lifted. A
// Relation used as a SQL placeholder already carries its typed predicates.
const RELATION_BINDS: &str = r##"
Widget.connection.execute("INSERT INTO widgets (id, number) VALUES (0, 0), (1, 7), (2, NULL)")
expect_ids = ->(expected, relation) do
  actual = relation.order(:id).pluck(:id)
  raise "#{relation.to_sql}: #{actual.inspect}, expected #{expected.inspect}" unless actual == expected
end
value = ["7", "suffix"].join
inner = Widget.where({"number" => value})
expect_ids.call([1], Widget.where("widgets.id IN (?)", inner.select(:id)))
expect_ids.call([1], Widget.where("widgets.id IN (:ids)", ids: inner.select(:id)))
expect_ids.call([], Widget.where("widgets.id IN (?)", Widget.where(number: "not-an-export").select(:id)))
expect_ids.call([2], Widget.where("widgets.id IN (?)", Widget.where(number: nil).select(:id)))
raise "integer serialization missing" unless inner.to_sql.include?(" = 7")
if ActiveRecord::Relation.respond_to?(:lift_binds)
  lifted = ActiveRecord::Relation.lift_binds(inner.bound_sql)
  raise "cast must precede integer binding: #{lifted.inspect}" unless lifted.drop(2) == ["i", "7"]
  invalid = ActiveRecord::Relation.lift_binds(Widget.where(number: "not-an-export").bound_sql)
  raise "invalid integer text bound" unless invalid.length == 2 && invalid[0].include?(" = NULL")
end
puts "integer query casts compose with Relation binds"
"##;

#[test]
fn integer_query_casts_compose_with_relation_binds() {
    assert_query_parity(app(), MODEL_BODY, RELATION_BINDS);
}

// Sam's integer-column boolean conversion runs before the String list
// serializer; original cardinality, NULL and range arms still matter.
const BOOLEAN_STRING_QUERIES: &str = r##"
Widget.connection.execute("INSERT INTO widgets (id, number) VALUES (0, 0), (1, 1), (2, NULL), (7, 7)")
cases = [
  [true, [1], [0, 7]],
  [false, [0], [1, 7]],
  [[true, "x"], [1], [0, 7]],
  [[false, "1suffix"], [0, 1], [7]],
  [[true, false, "x"], [0, 1], [7]],
  [[true, "x", nil], [1, 2], [0, 7]],
  [[true, 0..0], [0, 1], [7]],
  [[true, "9223372036854775808"], [1], [0, 7]]
]
cases.each do |value, positive, negative|
  conditions = {"number" => value}
  actual = [Widget.where(conditions).order(:id).pluck(:id), Widget.where.not(conditions).order(:id).pluck(:id)]
  expected = [positive, negative]
  raise "boolean/String composition #{value.inspect}: #{actual.inspect}, expected #{expected.inspect}" unless actual == expected
  puts "#{value.inspect}: #{actual.inspect}"
end
combined = Widget.where(number: "1suffix").and(Widget.where(number: true)).order(:id).pluck(:id)
raise "String/boolean conjunction: #{combined.inspect}" unless combined == [1]
puts "String and boolean constraints compose"
"##;

#[test]
fn integer_query_strings_keep_integer_boolean_casts() {
    assert_query_parity(app(), MODEL_BODY, BOOLEAN_STRING_QUERIES);
}

fn native_review_control(schema: &str, models: &str, script: &str) {
    let oracle =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let native = format!(
        r##"
require {oracle:?}
unless RailsOracle.available?("activerecord", sqlite3: ">= 2.9")
  warn "skipping native review control: #{{RailsOracle.label}} or sqlite3 unavailable"
  exit 77
end
RailsOracle.activate("activerecord", sqlite3: ">= 2.9")
require "active_record"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
{schema}
{models}
{script}
"##
    );
    let output = emit_and_run::ruby()
        .args(["-e", &native])
        .output()
        .expect("Rails review control");
    if output.status.code() == Some(77) {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn integer_query_columns(source: &str) -> &str {
    source
        .split("def self.schema_integer_query_columns")
        .nth(1)
        .expect("integer query evidence")
        .split("\n  end")
        .next()
        .unwrap()
}

#[test]
fn query_cast_evidence_excludes_every_attribute_and_normalizes_declaration() {
    let cases = [
        "attribute :number, ActiveRecord::Type::String.new",
        "attribute :number, :string, limit: 8",
        "attribute \"number\", :string",
        "self.attribute :number, :string",
        "Widget.attribute :number, :string",
        "def self.zero_to_nil(n); n == 0 ? nil : n; end\n  normalizes :number, with: method(:zero_to_nil)",
        "normalizes :number, with: ->(n) { n == 0 ? nil : n }, apply_to_nil: [false].first",
        "normalizes \"number\", with: ->(n) { n == 0 ? nil : n }",
        "self.normalizes :number, with: ->(n) { n == 0 ? nil : n }",
        "Widget.normalizes :number, with: ->(n) { n == 0 ? nil : n }",
    ];
    // Run Rails first, including forms whose complete lowering remains a gap.
    for body in cases {
        let positive = if body.contains("normalizes") {
            "[1]"
        } else {
            "[]"
        };
        native_review_control(
            "ActiveRecord::Schema.define { create_table(:widgets) { |t| t.integer :number; t.integer :phase; t.string :label } }",
            &format!("class Widget < ActiveRecord::Base\n{body}\nend"),
            &format!(
                r##"
Widget.connection.execute("INSERT INTO widgets(id,number) VALUES(0,0),(1,1),(2,NULL)")
raise unless Widget.where(number: "1suffix").order(:id).pluck(:id) == {positive}
raise unless Widget.where.not(number: "abc").order(:id).pluck(:id) == [0,1]
"##
            ),
        );
    }
    let mut failures = Vec::new();
    for body in cases {
        let (emitted, _errors) = app()
            .write(
                "app/models/widget.rb",
                &format!("class Widget < ApplicationRecord\n{body}\nend\n"),
            )
            .emit(roundhouse::project::BuildTarget::Ruby);
        let source = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
        let columns = integer_query_columns(&source);
        assert!(
            columns.contains(":id") && columns.contains(":phase"),
            "unrelated columns lost: {body}: {columns}"
        );
        if columns.contains(":number") {
            failures.push(format!("{body}: {columns}"));
        }
    }
    assert!(
        failures.is_empty(),
        "decorated columns retained plain integer evidence:\n{}",
        failures.join("\n")
    );
    for body in &cases[..4] {
        app()
            .write(
                "app/models/widget.rb",
                &format!("class Widget < ApplicationRecord\n{body}\nend\n"),
            )
            .run_ruby(
                r##"
Widget.connection.execute("INSERT INTO widgets(id,number) VALUES(0,0),(1,1),(2,NULL)")
raise unless Widget.where(number: "1suffix").order(:id).pluck(:id) == []
raise unless Widget.where.not(number: "abc").order(:id).pluck(:id) == [0,1]
"##,
            )
            .assert_passes();
    }
}

#[test]
fn query_cast_evidence_handles_multiple_and_computed_declaration_names() {
    let multi = "def self.zero_to_nil(n); n == 0 ? nil : n; end\n  normalizes :number, :phase, with: method(:zero_to_nil)";
    native_review_control(
        "ActiveRecord::Schema.define { create_table(:widgets) { |t| t.integer :number; t.integer :phase } }",
        &format!("class Widget < ActiveRecord::Base\n{multi}\nend"),
        r##"Widget.connection.execute("INSERT INTO widgets(id,number,phase) VALUES(0,0,0),(1,1,1),(2,NULL,NULL)")
raise unless Widget.where.not(number: "abc").pluck(:id) == [0,1]
raise unless Widget.where.not(phase: "abc").pluck(:id) == [0,1]
"##,
    );
    let computed =
        "attribute ENV.fetch(\"ROUNDHOUSE_REVIEW_ATTRIBUTE_COLUMN\", \"number\"), :string";
    native_review_control(
        "ActiveRecord::Schema.define { create_table(:widgets) { |t| t.integer :number; t.integer :phase } }",
        &format!("class Widget < ActiveRecord::Base\n{computed}\nend"),
        r##"Widget.connection.execute("INSERT INTO widgets(id,number) VALUES(0,0),(1,1),(2,NULL)")
raise unless Widget.where(number: "1suffix").pluck(:id) == []
"##,
    );
    for (body, unknown) in [(multi, false), (computed, true)] {
        let (emitted, _errors) = app()
            .write(
                "app/models/widget.rb",
                &format!("class Widget < ApplicationRecord\n{body}\nend\n"),
            )
            .emit(roundhouse::project::BuildTarget::Ruby);
        let source = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
        let columns = integer_query_columns(&source);
        assert!(
            !columns.contains(":number") && !columns.contains(":phase"),
            "{body}: {columns}"
        );
        if unknown {
            assert!(
                !columns.contains(":id"),
                "unknown declaration target: {columns}"
            );
        } else {
            assert!(columns.contains(":id"), "{columns}");
        }
    }
}

const REFERENCE_SCHEMA: &str = r##"ActiveRecord::Schema.define do
  create_table(:owners, id: :string)
  create_table(:uuid_owners, id: :uuid)
  create_table(:widgets) do |t|
    t.references :owner, type: :string
    t.belongs_to :uuid_owner, type: :uuid
    t.references :integer_owner, type: :bigint
  end
end
"##;
const REFERENCE_MODELS: &str = "class Owner < ActiveRecord::Base; end\nclass UuidOwner < ActiveRecord::Base; end\nclass Widget < ActiveRecord::Base; end\n";
const REFERENCE_QUERIES: &str = r##"
Widget.connection.execute("INSERT INTO widgets(id,owner_id,uuid_owner_id,integer_owner_id) VALUES(1,'7suffix','a0000000-0000-0000-0000-000000000007',7),(2,'other','a0000000-0000-0000-0000-000000000008',8)")
checks = [
  [Widget.where(owner_id: "7suffix"), [1]],
  [Widget.where.not(owner_id: "7suffix"), [2]],
  [Widget.where(uuid_owner_id: "a0000000-0000-0000-0000-000000000007"), [1]],
  [Widget.where.not(uuid_owner_id: "a0000000-0000-0000-0000-000000000007"), [2]],
  [Widget.where(integer_owner_id: "7suffix"), [1]]
]
checks.each do |relation, expected|
  actual = relation.order(:id).pluck(:id)
  raise "#{relation.to_sql}: #{actual.inspect}" unless actual == expected
end
puts "reference query types match"
"##;

fn reference_query_app() -> emit_and_run::Overlay {
    app()
        .write("db/schema.rb", REFERENCE_SCHEMA)
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord; end\n",
        )
        .write(
            "app/models/owner.rb",
            "class Owner < ApplicationRecord; end\n",
        )
        .write(
            "app/models/uuid_owner.rb",
            "class UuidOwner < ApplicationRecord; end\n",
        )
}

const REFERENCE_MIGRATION: &str = r##"class CreateReferenceColumns < ActiveRecord::Migration[8.1]
  def change
    create_table(:owners, id: :string)
    create_table(:uuid_owners, id: :uuid)
    create_table(:widgets)
    add_reference :widgets, :owner, type: :string
    add_belongs_to :widgets, :uuid_owner, type: :uuid
    add_reference :widgets, :integer_owner, type: :bigint
  end
end
"##;

fn reference_migration_app() -> emit_and_run::Overlay {
    reference_query_app().remove("db/schema.rb").write(
        "db/migrate/001_create_reference_columns.rb",
        REFERENCE_MIGRATION,
    )
}

#[test]
fn reference_queries_keep_the_declared_column_type() {
    native_review_control(REFERENCE_SCHEMA, REFERENCE_MODELS, REFERENCE_QUERIES);
    let adds = REFERENCE_MIGRATION
        .replace(
            "class CreateReferenceColumns < ActiveRecord::Migration[8.1]",
            "ActiveRecord::Schema.define do",
        )
        .replace("  def change", "");
    let adds = adds.strip_suffix("end\n").unwrap();
    native_review_control(adds, REFERENCE_MODELS, REFERENCE_QUERIES);
    let factories: [fn() -> emit_and_run::Overlay; 2] =
        [reference_query_app, reference_migration_app];
    let mut failures = Vec::new();
    for factory in factories {
        let (emitted, errors) = factory().emit(roundhouse::project::BuildTarget::Ruby);
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let source = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
        let columns = integer_query_columns(&source);
        if !columns.contains(":integer_owner_id")
            || columns.contains(":owner")
            || columns.contains(":uuid_owner")
        {
            failures.push(columns.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "references lost their declared column types: {failures:?}"
    );
    for factory in factories {
        factory().run_ruby(REFERENCE_QUERIES).assert_passes();
    }
}

#[test]
fn reference_query_type_is_not_inferred_from_the_target_key() {
    // Reference IR denotes integer storage, as Rails' default reference does.
    native_review_control(
        r##"ActiveRecord::Schema.define do
  create_table(:owners, id: :string)
  create_table(:uuid_owners, id: :uuid)
  create_table(:widgets) { |t| t.references :owner; t.references :uuid_owner }
end"##,
        REFERENCE_MODELS,
        r##"
raise unless Widget.columns_hash["owner_id"].type == :integer
raise unless Widget.columns_hash["uuid_owner_id"].type == :integer
Widget.connection.execute("INSERT INTO widgets(id,owner_id,uuid_owner_id) VALUES(1,7,7)")
raise unless Widget.where(owner_id: "7suffix").pluck(:id) == [1]
raise unless Widget.where(uuid_owner_id: "7suffix").pluck(:id) == [1]
"##,
    );
    let (emitted, errors) = app().write("db/schema.rb",
        "ActiveRecord::Schema.define do\n  create_table(:owners, id: :string)\n  create_table(:uuid_owners, id: :uuid)\n  create_table(:widgets) { |t| t.references :owner; t.references :uuid_owner }\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord; end\n")
        .write("app/models/owner.rb", "class Owner < ApplicationRecord; end\n")
        .write("app/models/uuid_owner.rb", "class UuidOwner < ApplicationRecord; end\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let source = std::fs::read_to_string(emitted.join("app/models/widget.rb")).unwrap();
    let columns = integer_query_columns(&source);
    assert!(
        columns.contains(":owner") && columns.contains(":uuid_owner"),
        "default references are integer columns: {columns}"
    );
}
