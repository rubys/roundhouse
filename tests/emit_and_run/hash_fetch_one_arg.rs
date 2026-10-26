use super::emit_and_run;

/// #1014: one-argument `Hash#fetch` answers the value or raises
/// `KeyError`, never nil, so it is typed the value. It was typed
/// `value | nil`, which made `roles.fetch(a) >= roles.fetch(b)` and
/// `RANKS.fetch(k) + 1` `incompatible_binop` errors, though Rails runs
/// them. Answers are Rails 8.1.4's for the same model and records.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"memberships\", force: :cascade do |t|\n    t.integer \"role\", default: 0, null: false\n    t.string \"tier\"\n  end\nend\n",
        )
        .write(
            "app/models/membership.rb",
            r#"class Membership < ApplicationRecord
  RANKS = { "viewer" => 0, "editor" => 1, "owner" => 2 }.freeze

  enum :role, { viewer: 0, editor: 1, owner: 2 }

  def allows?(level)
    self.class.roles.fetch(role) >= self.class.roles.fetch(level.to_s)
  end

  def rank
    self.class.roles.fetch(role) + 1
  end

  def tier_rank
    RANKS.fetch(tier) + 1
  end
end
"#,
        )
}

#[test]
fn one_argument_fetch_answers_the_value_like_rails() {
    app()
        .run_ruby(
            r##"m = Membership.new
m.role = :editor
m.tier = "owner"
got = [m.allows?(:viewer), m.allows?(:owner), m.rank, m.tier_rank]
raise "values: #{got.inspect}" unless got == [true, false, 2, 3]
m.tier = "guest"
missing = begin
  m.tier_rank
  :no_raise
rescue KeyError => e
  e.message
end
raise "missing key: #{missing.inspect}" unless missing == "key not found: \"guest\""
puts "hash fetch passed"
"##,
        )
        .assert_passes();
}

/// TypeScript raises on a missing key, as Ruby does, instead of reading
/// `undefined` (the one-argument fetch is now typed the value). The
/// receiver is a Hash-typed local; a constant read does not carry the
/// Hash type into the TypeScript emit, a separate gap.
#[test]
fn typescript_one_argument_fetch_raises_on_a_missing_key() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"memberships\", force: :cascade do |t|\n    t.string \"tier\"\n  end\nend\n",
        )
        .write(
            "app/models/membership.rb",
            "class Membership < ApplicationRecord\n  def tier_rank\n    ranks = { \"viewer\" => 0, \"owner\" => 2 }\n    ranks.fetch(tier) + 1\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:#?}");
    let model = std::fs::read_to_string(emitted.join("app/models/membership.ts")).expect("membership.ts");
    assert!(model.contains("throw new Error(\"key not found: \" + k)"), "{model}");
    // An own key: an Object.prototype name (`"constructor"`) is not one.
    assert!(model.contains("if (!Object.prototype.hasOwnProperty.call(h, k))"), "{model}");
}
