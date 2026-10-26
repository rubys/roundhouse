//! #1158: on the ruby-family targets, the public writer of an integer,
//! float, decimal or boolean column casts what it is given, as an
//! ActiveRecord attribute does. Before, `record.min_favs = "5"` read back
//! `"5"`, so a request param reached every validator, comparison and
//! serializer as a String, and `numericality:` called it "is not a
//! number". The table below is what activerecord 8.1.4 answers
//! (`rails_answers_the_cast_table` checks Rails against it); a decimal
//! column is a Float in the lowered app, so its values compare as floats.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

use std::process::Command;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "policies", force: :cascade do |t|
    t.integer "min_favs"
    t.float "ratio"
    t.decimal "price"
    t.boolean "active"
  end
  create_table "gauges", force: :cascade do |t|
    t.integer "min_favs"
    t.integer "age"
    t.float "rating"
    t.integer "level"
  end
end
"#;

/// The issue's model, plus a float `only_integer` column and an
/// `allow_nil:` one, for `numericality:` reading the user's raw value.
const GAUGE: &str = r#"class Gauge < ApplicationRecord
  validates :min_favs, numericality: { greater_than_or_equal_to: 1 }
  validates :age, numericality: { only_integer: true }
  validates :rating, numericality: { only_integer: true }
  validates :level, numericality: true, allow_nil: true
end
"#;

/// `(Ruby building a Gauge g, its error messages)`. Rails reads the
/// value the user assigned (`<attr>_before_type_cast`) where it came from
/// the user, the cast value otherwise, and parses it as
/// `NumericalityValidator#parse_as_number` does; `allow_nil` tests the
/// cast value.
const RAW: &[(&str, &str)] = &[
    // The issue's table: `min_favs` (>= 1), with the others valid.
    (r#"g = Gauge.new(min_favs: nil, age: 3, rating: 1, level: 1)"#, r#"["Min favs is not a number"]"#),
    (r#"g = Gauge.new(min_favs: "", age: 3, rating: 1, level: 1)"#, r#"["Min favs is not a number"]"#),
    (r#"g = Gauge.new(min_favs: "abc", age: 3, rating: 1, level: 1)"#, r#"["Min favs is not a number"]"#),
    (r#"g = Gauge.new(min_favs: "0", age: 3, rating: 1, level: 1)"#, r#"["Min favs must be greater than or equal to 1"]"#),
    (r#"g = Gauge.new(min_favs: "5", age: 3, rating: 1, level: 1)"#, "[]"),
    (r#"g = Gauge.new(min_favs: 5, age: 3, rating: 1, level: 1)"#, "[]"),
    (r#"g = Gauge.new(min_favs: "5.5", age: 3, rating: 1, level: 1)"#, "[]"),
    (r#"g = Gauge.new(min_favs: " ", age: 3, rating: 1, level: 1)"#, r#"["Min favs is not a number"]"#),
    // `age` (only_integer), with the others valid.
    (r#"g = Gauge.new(min_favs: 2, age: "abc", rating: 1, level: 1)"#, r#"["Age is not a number"]"#),
    (r#"g = Gauge.new(min_favs: 2, age: "5", rating: 1, level: 1)"#, "[]"),
    (r#"g = Gauge.new(min_favs: 2, age: "5.5", rating: 1, level: 1)"#, r#"["Age must be an integer"]"#),
    (r#"g = Gauge.new(min_favs: 2, age: 5.5, rating: 1, level: 1)"#, r#"["Age must be an integer"]"#),
    // A float column holding a whole number: the raw Integer 2 passes
    // only_integer, a raw Float 2.0 does not.
    (r#"g = Gauge.new(min_favs: 2, age: 3, rating: 2, level: 1)"#, "[]"),
    (r#"g = Gauge.new(min_favs: 2, age: 3, rating: 2.0, level: 1)"#, r#"["Rating must be an integer"]"#),
    (r#"g = Gauge.new(min_favs: 2, age: 3, rating: "2", level: 1)"#, "[]"),
    // allow_nil tests the cast value: a blank String is nil there.
    (r#"g = Gauge.new(min_favs: 2, age: 3, rating: 1, level: "")"#, "[]"),
    (r#"g = Gauge.new(min_favs: 2, age: 3, rating: 1, level: " ")"#, "[]"),
    (r#"g = Gauge.new(min_favs: 2, age: 3, rating: 1, level: "abc")"#, r#"["Level is not a number"]"#),
    // nil assigned after a String: the raw value is nil, then the cast nil.
    (r#"g = Gauge.new(min_favs: "abc", age: 3, rating: 1, level: 1); g.min_favs = nil"#, r#"["Min favs is not a number"]"#),
    // A loaded record did not come from the user: the stored value counts.
    // The Integer 2 assigned above passed only_integer; reloaded, the
    // float column holds 2.0, which does not.
    (r#"g = Gauge.new(min_favs: "7", age: 3, rating: 2, level: 1); g.save!; g = Gauge.find(g.id)"#, r#"["Rating must be an integer"]"#),
];

const MODEL: &str = r#"class Policy < ApplicationRecord
  validates :min_favs, numericality: { greater_than_or_equal_to: 1 }, allow_nil: true
end
"#;

/// `(value, [min_favs, ratio, price, active])` after `record.col = value`.
const CASTS: &[(&str, &str)] = &[
    ("nil", "[nil, nil, nil, nil]"),
    (r#""""#, "[nil, nil, nil, nil]"),
    (r#"" ""#, "[nil, nil, nil, true]"),
    (r#""abc""#, "[0, 0.0, 0.0, true]"),
    (r#""0""#, "[0, 0.0, 0.0, false]"),
    (r#""5""#, "[5, 5.0, 5.0, true]"),
    (r#"" 5 ""#, "[5, 5.0, 5.0, true]"),
    (r#""5.5""#, "[5, 5.5, 5.5, true]"),
    (r#""1e3""#, "[1, 1000.0, 1000.0, true]"),
    (r#""0x1A""#, "[0, 0.0, 0.0, true]"),
    (r#""1_000""#, "[1000, 1000.0, 1000.0, true]"),
    (r#""f""#, "[0, 0.0, 0.0, false]"),
    (r#""off""#, "[0, 0.0, 0.0, false]"),
    ("true", "[1, 1.0, 1.0, true]"),
    ("false", "[0, 0.0, 0.0, false]"),
    ("5", "[5, 5.0, 5.0, true]"),
    ("5.7", "[5, 5.7, 5.7, true]"),
];

/// `(value, [min_favs, ratio, price, active])` for a value that is not
/// nil, a boolean, a number or a String: `Helpers::Numeric#cast` makes a
/// blank one (`[]`, `{}`, `:""`, `Set[]`) nil, then `Type::Integer` answers
/// `to_i rescue nil`, `Type::Float` an unrescued `to_f` (a Symbol, an
/// Array, a Hash and an Object raise NoMethodError), and `Type::Decimal`
/// what the value's `to_s` reads. A Duration casts as its seconds. `Type::Boolean` makes only `""` nil.
const OTHER: &[(&str, &str)] = &[
    (":many", r#"["nil", "NoMethodError", "0.0", "true"]"#),
    (":off", r#"["nil", "NoMethodError", "0.0", "false"]"#),
    (r#":"5""#, r#"["nil", "NoMethodError", "5.0", "true"]"#),
    (r#":"""#, r#"["nil", "nil", "nil", "true"]"#),
    ("[]", r#"["nil", "nil", "nil", "true"]"#),
    ("[1]", r#"["nil", "NoMethodError", "0.0", "true"]"#),
    ("{}", r#"["nil", "nil", "nil", "true"]"#),
    ("{ a: 1 }", r#"["nil", "NoMethodError", "0.0", "true"]"#),
    ("Object.new", r#"["nil", "NoMethodError", "0.0", "true"]"#),
    ("Set[]", r#"["nil", "nil", "nil", "true"]"#),
    ("Set[1]", r#"["nil", "NoMethodError", "0.0", "true"]"#),
    ("ActiveSupport::Duration.minutes(5)", r#"["300", "300.0", "300.0", "true"]"#),
];

/// Ruby asserting `OTHER`: each column is assigned on its own, and a
/// raise reads as its class.
fn other_assertions() -> String {
    let mut s = String::new();
    for (value, want) in OTHER {
        s.push_str(&format!("v = {value}
got = []
"));
        for col in ["min_favs", "ratio", "price", "active"] {
            let read = if col == "price" { "x.price.nil? ? \"nil\" : x.price.to_f.inspect".to_string() } else { format!("x.{col}.inspect") };
            s.push_str(&format!(
                "begin
  x = Policy.new
  x.{col} = v
  got << ({read})
rescue NoMethodError => e
  got << e.class.name
end
"
            ));
        }
        s.push_str(&format!(
            "raise %q(other {}: ) + got.inspect unless got.inspect == {want:?}
",
            value.replace(['(', ')'], "")
        ));
    }
    s
}

/// `numericality: { greater_than_or_equal_to: 1 }` on a String param, as
/// `Policy.new(min_favs: value)`: the cast value is what is compared.
const NUMERICALITY: &[(&str, &str)] = &[
    (r#""0""#, r#"["Min favs must be greater than or equal to 1"]"#),
    (r#""5""#, "[]"),
    (r#""5.5""#, "[]"),
];

/// Ruby asserting both tables. `$errors` answers a record's full messages,
/// as each side spells it.
fn assertions() -> String {
    let mut s = String::new();
    for (value, want) in CASTS {
        s.push_str(&format!(
            "x = Policy.new\nx.min_favs = {value}\nx.ratio = {value}\nx.price = {value}\nx.active = {value}\ngot = [x.min_favs, x.ratio, x.price.nil? ? nil : x.price.to_f, x.active].inspect\nraise \"{}: #{{got}}\" unless got == {want:?}\n",
            value.replace('"', "\\\"")
        ));
    }
    for (setup, want) in RAW {
        s.push_str(&format!(
            "{setup}\ng.valid?\ngot = messages(g).inspect\nraise %q({}: ) + got unless got == {want:?}\n",
            setup.replace(['(', ')'], "")
        ));
    }
    for (value, want) in NUMERICALITY {
        s.push_str(&format!(
            "x = Policy.new(min_favs: {value})\nx.valid?\ngot = messages(x).inspect\nraise \"numericality {}: #{{got}}\" unless got == {want:?}\n",
            value.replace('"', "\\\"")
        ));
    }
    s.push_str(&other_assertions());
    s
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/policy.rb", MODEL)
        .write("app/models/gauge.rb", GAUGE)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
}

#[test]
fn the_ruby_target_casts_each_typed_writer_as_rails_does() {
    app()
        .run_ruby(&format!("def messages(r)\n  r.errors\nend\n{}puts \"PASS\"\n", assertions()))
        .assert_passes();
}

/// A loaded record keeps the stored values: hydration passes values of the
/// column's type, on which the cast is the identity.
#[test]
fn a_saved_and_reloaded_record_reads_its_stored_values() {
    app()
        .run_ruby(
            r#"x = Policy.new
x.min_favs = "7"
x.ratio = "2.5"
x.price = "9.25"
x.active = "1"
x.save!
y = Policy.find(x.id)
got = [y.min_favs, y.ratio, y.price, y.active].inspect
raise "reloaded: #{got}" unless got == "[7, 2.5, 9.25, true]"
puts "PASS"
"#,
        )
        .assert_passes();
}

/// The tables themselves, against activerecord 8.1.4 on sqlite3.
#[test]
fn rails_answers_the_cast_table() {
    let probe = r#"gem "activerecord", "8.1.4"; require "active_record"; require "sqlite3""#;
    if !Command::new("ruby").args(["-e", probe]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipping: ruby with activerecord 8.1.4 and sqlite3 not available");
        return;
    }
    let script = format!(
        r##"gem "activerecord", "8.1.4"
require "active_record"
require "sqlite3"
ActiveRecord::Base.establish_connection(adapter: "sqlite3", database: ":memory:")
ActiveRecord::Schema.verbose = false
ActiveRecord::Schema.define do
  create_table(:policies) do |t|
    t.integer :min_favs
    t.float :ratio
    t.decimal :price
    t.boolean :active
  end
  create_table(:gauges) do |t|
    t.integer :min_favs
    t.integer :age
    t.float :rating
    t.integer :level
  end
end
class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
{MODEL}
{GAUGE}
def messages(r)
  r.errors.full_messages
end
{asserts}puts "Rails agrees"
"##,
        asserts = assertions(),
    );
    let output = Command::new("ruby").arg("-e").arg(&script).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("Rails agrees"),
        "activerecord 8.1.4 disagrees with the table\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
}
