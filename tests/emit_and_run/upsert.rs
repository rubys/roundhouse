//! `Model.upsert(attrs, on_duplicate: …)`, as lobsters' Keystore counts:
//! one statement that inserts, or folds into an UPDATE on the unique key.
//! On a table with timestamps it stamps them as Rails 8.1 does: both on
//! insert, `updated_at` again only when an assigned column changed.

use super::emit_and_run;

fn keystore_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|\n",
            "create_table \"keystores\", force: :cascade do |t|\n    t.string \"key\", null: false\n    t.bigint \"value\"\n    t.index [\"key\"], name: \"index_keystores_on_key\", unique: true\n  end\n\n  create_table \"comments\", force: :cascade do |t|\n",
        )
        .write(
            "app/models/keystore.rb",
            "class Keystore < ApplicationRecord\n  def self.value_for(key)\n    where(key: key).pick(:value)\n  end\n\n  def self.put(key, value)\n    Keystore.upsert({key: key, value: value}, unique_by: :key, returning: false)\n    true\n  end\n\n  def self.incremented_value_for(key, amount = 1)\n    Keystore.transaction do\n      Keystore.upsert({key: key, value: amount}, unique_by: :key, on_duplicate: Arel.sql(\"value = value + 1\"))\n      value_for(key)\n    end\n  end\nend\n",
        )
        .write(
            "test/models/keystore_test.rb",
            r#"require "test_helper"

class KeystoreTest < ActiveSupport::TestCase
  test "upsert inserts, then folds into an update on the key" do
    assert Keystore.put("hits", 5)
    assert_equal 5, Keystore.value_for("hits")
    Keystore.put("hits", 7)
    assert_equal 7, Keystore.value_for("hits")
    assert_equal 8, Keystore.incremented_value_for("hits")
    assert_equal 1, Keystore.incremented_value_for("fresh")
    assert_equal 2, Keystore.count
  end
end
"#,
        )
}

#[test]
fn upsert_inserts_then_updates_on_the_unique_key() {
    keystore_app().run_test("test/models/keystore_test.rb").assert_passes();
}

#[test]
fn upsert_stamps_timestamps_as_rails_does() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.put(id, title)\n    Article.upsert({id: id, title: title, body: \"b\"})\n  end\n",
        )
        .write(
            "test/models/article_upsert_test.rb",
            r#"require "test_helper"

class ArticleUpsertTest < ActiveSupport::TestCase
  test "timestamps are stamped on insert and moved only by a change" do
    Article.put(500, "First")
    stamped = Article.find(500)
    assert stamped.created_at.present?
    assert_equal stamped.created_at, stamped.updated_at
    sleep 0.01
    Article.put(500, "First")
    unchanged = Article.find(500)
    assert_equal stamped.updated_at, unchanged.updated_at
    sleep 0.01
    Article.put(500, "Second")
    changed = Article.find(500)
    assert_equal "Second", changed.title
    assert_equal stamped.created_at, changed.created_at
    assert changed.updated_at > stamped.updated_at
    Article.upsert({"id" => 501, "title" => "Given", "body" => "b", "created_at" => "2020-01-01 00:00:00", "updated_at" => "2020-01-01 00:00:00"})
    given = Article.find(501)
    assert_equal "Given", given.title
    assert_equal 2020, given.created_at.year
    Article.upsert({"id" => 501, "title" => "Moved", "body" => "b", "updated_at" => "2021-01-01 00:00:00"})
    assert_equal 2021, Article.find(501).updated_at.year
  end
end
"#,
        )
        .run_test("test/models/article_upsert_test.rb")
        .assert_passes();
}

/// `record_timestamps: false` on the call leaves them as the row has them,
/// as Rails' does.
#[test]
fn upsert_without_record_timestamps_leaves_them_unset() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|\n",
            "create_table \"visits\", force: :cascade do |t|\n    t.string \"path\"\n    t.datetime \"created_at\"\n    t.datetime \"updated_at\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|\n",
        )
        .write("app/models/visit.rb", "class Visit < ApplicationRecord\n  def self.put(id, path)\n    Visit.upsert_all([{id: id, path: path}], record_timestamps: false)\n  end\nend\n")
        .write(
            "test/models/visit_test.rb",
            "require \"test_helper\"\n\nclass VisitTest < ActiveSupport::TestCase\n  test \"no timestamps when the call says so\" do\n    Visit.put(1, \"/a\")\n    visit = Visit.find(1)\n    assert_equal \"/a\", visit.path\n    assert_nil visit.created_at\n    assert_nil visit.updated_at\n  end\nend\n",
        )
        .run_test("test/models/visit_test.rb")
        .assert_passes();
}

fn quiet_stamping_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true\n\n  def self.quietly(title)\n    Article.record_timestamps = false\n    Article.create!(title: title, body: \"Long enough body text\")\n  ensure\n    Article.record_timestamps = true\n  end\n",
    )
}

/// Rails' `record_timestamps = false` turns a model's stamping off until
/// it is turned back on, and a subclass inherits it until it sets its own.
#[test]
fn turning_a_model_s_stamping_off_leaves_its_saves_unstamped() {
    quiet_stamping_app()
        .write(
            "test/models/quiet_stamping_test.rb",
            "require \"test_helper\"\n\nclass QuietStampingTest < ActiveSupport::TestCase\n  test \"a save while stamping is off stamps nothing\" do\n    quiet = Article.quietly(\"Quiet\")\n    assert_nil quiet.reload.created_at\n    assert_nil quiet.updated_at\n    assert Article.record_timestamps\n    loud = Article.create!(title: \"Loud\", body: \"Long enough body text\")\n    assert_not_nil loud.reload.created_at\n  end\n\n  test \"a subclass inherits the switch until it sets its own\" do\n    ApplicationRecord.record_timestamps = false\n    assert_not Comment.record_timestamps\n    Comment.record_timestamps = true\n    assert Comment.record_timestamps\n    assert_not ApplicationRecord.record_timestamps\n  ensure\n    ApplicationRecord.record_timestamps = true\n    Comment.record_timestamps = true\n  end\nend\n",
        )
        .run_test("test/models/quiet_stamping_test.rb")
        .assert_passes();
}

/// Only the ruby family keeps the switch: a strict target reports it, and
/// an app's own `record_timestamps=` on an instance is no model's switch.
#[test]
fn turning_a_model_s_stamping_off_is_reported_on_a_strict_target() {
    let (_emitted, errors) = quiet_stamping_app().emit(roundhouse::project::BuildTarget::Rust);
    let found: Vec<&String> = errors.iter().filter(|e| e.contains("ActiveRecord::Base.record_timestamps= not supported (rust)")).collect();
    assert_eq!(found.len(), 2, "the `= false` and the `= true` that restores it: {errors:#?}");
    let (_emitted, errors) = quiet_stamping_app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("record_timestamps")), "{errors:#?}");
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  def self.stamp_again\n    Article.record_timestamps = true\n  end\n")
        .emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("ActiveRecord::Base.record_timestamps= not supported (rust)")), "{errors:#?}");
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/models/stamp_settings.rb", "class StampSettings\n  attr_accessor :record_timestamps\n\n  def self.quiet\n    settings = StampSettings.new\n    settings.record_timestamps = false\n    settings\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Rust);
    assert!(!errors.iter().any(|e| e.contains("record_timestamps")), "{errors:#?}");
}

/// No strict runtime carries `upsert_all`: the call is reported there.
#[test]
fn upsert_is_unsupported_on_strict_targets() {
    let (_emitted, errors) = keystore_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("`upsert` is the Ruby-family runtime's")), "{errors:#?}");
}

/// The bulk writes answer Rails' `ActiveRecord::Result`: `insert` and an
/// `insert_all` with `unique_by:` (an index name, here) are one INSERT in
/// the runtime, skipping a duplicate; `insert!` raises on one; `upsert`
/// assigns only what `update_only:` names.
#[test]
fn bulk_writes_answer_results_and_take_their_options() {
    keystore_app()
        .edit(
            "db/schema.rb",
            "    t.bigint \"value\"\n",
            "    t.bigint \"value\"\n    t.string \"note\"\n",
        )
        .write(
            "test/models/keystore_bulk_test.rb",
            r#"require "test_helper"

class KeystoreBulkTest < ActiveSupport::TestCase
  test "insert, insert_all with an index name, insert! and update_only" do
    first = Keystore.insert({ key: "a", value: 1, note: "x" })
    assert_equal 1, first.affected_rows
    assert_equal 1, first.rows.length
    skipped = Keystore.insert_all([{ key: "a", value: 2, note: "y" }, { key: "b", value: 3, note: "y" }], unique_by: :index_keystores_on_key)
    assert_equal 1, skipped.affected_rows
    assert_equal 1, Keystore.where(key: "a").pick(:value)
    assert_raises(ActiveRecord::RecordNotUnique) { Keystore.insert!({ key: "b", value: 4, note: "z" }) }
    upserted = Keystore.upsert({ key: "b", value: 5, note: "z" }, unique_by: :key, update_only: [:value])
    assert_equal 1, upserted.affected_rows
    assert_equal [5, "y"], Keystore.where(key: "b").pick(:value, :note)
    assert_equal 2, Keystore.count
  end
end
"#,
        )
        .run_test("test/models/keystore_bulk_test.rb")
        .assert_passes();
}

/// The two `insert_all` forms the lowering inlines, which a strict target
/// therefore does not report: the bare one skips a duplicate, and the
/// raising one answers the RETURNING columns.
#[test]
fn inlined_insert_all_forms_run() {
    keystore_app()
        .write(
            "test/models/keystore_inlined_insert_all_test.rb",
            r#"require "test_helper"

class KeystoreInlinedInsertAllTest < ActiveSupport::TestCase
  test "insert_all skips a duplicate and insert_all! returns ids" do
    Keystore.insert_all([{ key: "a", value: 1 }, { key: "b", value: 2 }])
    Keystore.insert_all([{ key: "a", value: 9 }])
    assert_equal 2, Keystore.count
    assert_equal 1, Keystore.where(key: "a").pick(:value)
    inserted = Keystore.insert_all!([{ key: "c", value: 3 }], returning: %w[id])
    assert_equal [Keystore.where(key: "c").pick(:id)], inserted.rows.flatten
  end
end
"#,
        )
        .run_test("test/models/keystore_inlined_insert_all_test.rb")
        .assert_passes();
}

/// The runtime's `insert`, `insert!` and the `insert_all` forms the
/// lowering declines are the Ruby family's too; the forms it inlines
/// compile on a strict target.
#[test]
fn insert_is_unsupported_on_strict_targets() {
    let (_emitted, errors) = keystore_app()
        .edit(
            "app/models/keystore.rb",
            "  def self.put(key, value)\n",
            "  def self.add(key, value)\n    Keystore.insert({key: key, value: value})\n    Keystore.insert!({key: key, value: value})\n    Keystore.insert_all([{key: key, value: value}], unique_by: :key)\n  end\n\n  def self.put(key, value)\n",
        )
        .emit(roundhouse::project::BuildTarget::Rust);
    for method in ["insert", "insert!", "insert_all"] {
        let message = format!("`{method}` is the Ruby-family runtime's");
        assert!(errors.iter().any(|e| e.contains(&message)), "{method}: {errors:#?}");
    }
}

#[test]
fn inlined_insert_all_is_supported_on_strict_targets() {
    let (_emitted, errors) = keystore_app()
        .edit(
            "app/models/keystore.rb",
            "  def self.put(key, value)\n",
            "  def self.add(key, value)\n    Keystore.insert_all([{key: key, value: value}])\n    Keystore.insert_all!([{key: key, value: value}], returning: %w[id])\n  end\n\n  def self.put(key, value)\n",
        )
        .emit(roundhouse::project::BuildTarget::Rust);
    assert!(!errors.iter().any(|e| e.contains("insert_all")), "{errors:#?}");
}

/// The lowering inlines only a call on a model constant it resolves: a
/// receiverless call in the model's own class method, or a rooted
/// constant, reaches the Ruby-family runtime and is reported.
#[test]
fn receiverless_and_rooted_inserts_are_unsupported_on_strict_targets() {
    let (_emitted, errors) = keystore_app()
        .edit(
            "app/models/keystore.rb",
            "  def self.put(key, value)\n",
            "  def self.add(key, value)\n    insert!({key: key, value: value})\n    insert_all([{key: key, value: value}])\n    ::Keystore.insert_all([{key: key, value: value}])\n  end\n\n  def self.put(key, value)\n",
        )
        .emit(roundhouse::project::BuildTarget::Rust);
    let reported = |method: &str| {
        let message = format!("`{method}` is the Ruby-family runtime's");
        errors.iter().filter(|e| e.contains(&message)).count()
    };
    assert_eq!(reported("insert!"), 1, "{errors:#?}");
    assert_eq!(reported("insert_all"), 2, "{errors:#?}");
}

/// A record's own integer key is written, as Rails writes it: `create`
/// with an `id:` and an inlined `insert_all` row's `id:` keep it, a row
/// whose key is taken is skipped, and a record given none still takes the
/// database's.
#[test]
fn a_supplied_integer_key_is_written() {
    keystore_app()
        .write(
            "test/models/keystore_key_test.rb",
            r#"require "test_helper"

class KeystoreKeyTest < ActiveSupport::TestCase
  test "a supplied id is the row's" do
    Keystore.create!(id: 900, key: "given", value: 1)
    assert_equal "given", Keystore.find(900).key
    Keystore.insert_all([{ id: 910, key: "bulk", value: 2 }])
    assert_equal "bulk", Keystore.find(910).key
    Keystore.insert_all([{ id: 910, key: "taken", value: 4 }])
    assert_equal "bulk", Keystore.find(910).key
    assert_not Keystore.exists?(key: "taken")
    assigned = Keystore.create!(key: "assigned", value: 3)
    assert assigned.id > 0
    assert_equal "assigned", Keystore.find(assigned.id).key
  end
end
"#,
        )
        .run_test("test/models/keystore_key_test.rb")
        .assert_passes();
}

/// A table with a generated column inserts through `RETURNING`; a supplied
/// integer key is written there too, and the generated value still hydrates.
#[test]
fn a_supplied_integer_key_is_written_beside_a_generated_column() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false\n",
            "    t.string \"title\"\n    t.text \"body\"\n    t.virtual \"display_text\", type: :string, as: \"coalesce(title, '') || ' / ' || coalesce(body, '')\", stored: true\n    t.datetime \"created_at\", null: false\n",
        )
        .write(
            "test/models/article_generated_key_test.rb",
            r#"require "test_helper"

class ArticleGeneratedKeyTest < ActiveSupport::TestCase
  test "a supplied id is written and the generated column hydrates" do
    given = Article.create!(id: 950, title: "Given", body: "Long enough body text")
    assert_equal 950, given.id
    assert_equal "Given / Long enough body text", given.display_text
    assert_equal "Given", Article.find(950).title
    assigned = Article.create!(title: "Assigned", body: "Long enough body text")
    assert assigned.id > 0
    assert_equal "Assigned / Long enough body text", Article.find(assigned.id).display_text
  end
end
"#,
        )
        .run_test("test/models/article_generated_key_test.rb")
        .assert_passes();
}
