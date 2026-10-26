use super::emit_and_run;

/// `inclusion: { in: NAME.keys }` and `inclusion: { in: NAME }` with
/// `NAME` a Hash literal constant. #1148 folds a list constant; a Hash
/// one was the reported gap. `NAME.keys` is the keys' list, and Rails'
/// `in: NAME` asks `NAME.include?(value)`, which for a Hash is
/// `key?(value)`. The keys keep their kind (a Symbol key never matches a
/// string column's value), and the values are never read.
///
/// `Hash#key?` compares with `eql?`, not `==`, so `{ 1 => … }` does not
/// hold `1.0` while `[1].include?(1.0)` does: a bare Hash with a numeric
/// key stays the gap, while its `.keys` list folds.
///
/// Answers are Rails 8.1.4's for the same model and records.
const MODEL: &str = r#"class Widget < ApplicationRecord
  ZONES = { "north" => { base: 1 }, "south" => { base: 2 } }.freeze
  TIERS = { gold: 3, silver: 2 }
  STEPS = { 1 => "one", 2 => "two" }.freeze

  validates :zone, inclusion: { in: ZONES.keys }
  validates :tier, inclusion: { in: TIERS }
  validates :size, exclusion: { in: ZONES }
  validates :scale, inclusion: { in: STEPS.keys }
  validates :area, inclusion: { in: Gadget::AREAS.keys }
end
"#;

const GADGET: &str = "class Gadget < ApplicationRecord\n  AREAS = { \"east\" => 1, \"west\" => 2 }.freeze\nend\n";

const APPLICATION_RECORD: &str =
    "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "zone"
    t.string "tier"
    t.string "size"
    t.float "scale"
    t.string "area"
  end
  create_table "gadgets", force: :cascade do |t|
    t.string "name"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("app/models/gadget.rb", GADGET)
        .write("db/schema.rb", SCHEMA)
        .write("app/models/widget.rb", MODEL)
}

#[test]
fn hash_constant_inclusion_reports_no_gap() {
    let warnings = app().ingest_warnings();
    assert!(!warnings.iter().any(|w| w.contains("validates ")), "{warnings:?}");
}

#[test]
fn hash_constant_inclusion_matches_rails() {
    app()
        .run_ruby(
            r#"def errors_for(**attrs)
  w = Widget.new(**attrs)
  w.valid?
  w.errors.to_a
end
got = [
  errors_for(zone: "north", tier: "gold", size: "x", scale: 1.0, area: "east"),
  errors_for(zone: "south", tier: :gold, size: "north", scale: 2, area: "west"),
  errors_for(zone: "east", tier: "silver", size: nil, scale: 3.0, area: "north"),
]
want = [
  ["Tier is not included in the list"],
  ["Tier is not included in the list", "Size is reserved"],
  ["Zone is not included in the list", "Tier is not included in the list", "Scale is not included in the list", "Area is not included in the list"],
]
raise "errors: #{got.inspect}" unless got == want
puts "hash constant inclusion passed"
"#,
        )
        .assert_passes();
}

#[test]
fn a_bare_hash_with_a_numeric_key_stays_the_gap() {
    let warnings = emit_and_run::empty_app()
        .write("app/models/application_record.rb", APPLICATION_RECORD)
        .write("db/schema.rb", SCHEMA)
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  STEPS = { 1 => \"one\" }.freeze\n\n  validates :scale, inclusion: { in: STEPS }\nend\n",
        )
        .ingest_warnings();
    assert!(warnings.iter().any(|w| w.contains("validates :scale, inclusion: in:")), "{warnings:?}");
}

#[test]
fn hash_constant_inclusion_emits_cleanly_on_every_backend_target() {
    use roundhouse::project::BuildTarget::*;
    for target in [Ruby, Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Crystal, Elixir, Roda] {
        let (_emitted, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:#?}");
    }
}
