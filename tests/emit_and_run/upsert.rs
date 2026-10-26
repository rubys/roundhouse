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
