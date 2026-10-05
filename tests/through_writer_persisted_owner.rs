//! Emitted-program regression for this fix (kept out of tests/emit_and_run.rs
//! so concurrent appends there do not conflict). Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `record.tags = [...]` on a PERSISTED owner writes the join rows at once,
/// as Rails' `has_many :through` collection writer does (only a new record
/// defers them to its save). The synthesized writer only staged them for
/// `_sync_tags` in `after_save`, so code that saves a record and THEN
/// assigns its tags (`entry.save!; entry.tags = tags`) answered with the
/// tags in memory while no join row was ever written. Silent loss.
#[test]
fn a_through_collection_writer_on_a_persisted_owner_writes_at_once() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|",
            "  create_table \"labels\", force: :cascade do |t|\n    t.string \"name\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"labelings\", force: :cascade do |t|\n    t.integer \"article_id\", null: false\n    t.integer \"label_id\", null: false\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"articles\", force: :cascade do |t|",
        )
        .write("app/models/label.rb", "class Label < ApplicationRecord\nend\n")
        .write(
            "app/models/labeling.rb",
            "class Labeling < ApplicationRecord\n  belongs_to :article\n  belongs_to :label\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  has_many :labelings, dependent: :destroy\n  has_many :labels, through: :labelings\n",
        )
        .run_ruby(r#"
Labeling.delete_all
Article.delete_all
Label.delete_all
a = Label.create!(name: "a")
b = Label.create!(name: "b")
saved = Article.create!(title: "Persisted", body: "Long enough body")
saved.labels = [a, b]
raise "persisted owner: #{Labeling.count} join rows, want 2" unless Labeling.count == 2
saved.labels = [b]
raise "replace: #{Labeling.count} join rows, want 1" unless Labeling.count == 1
fresh = Article.new(title: "Fresh", body: "Long enough body")
fresh.labels = [a]
raise "new owner wrote early" unless Labeling.count == 1
fresh.save!
raise "new owner after save: #{Labeling.count}, want 2" unless Labeling.count == 2
puts "through writer ok"
"#)
        .assert_passes();
}
