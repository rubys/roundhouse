//! Emitted-program regression for the `has_many :through` collection
//! writer when the associations name their classes top-level-anchored
//! (`class_name: "::Ns::Label"`) and the through association is
//! polymorphic (`as:`), the shape of a `Admin::Report::Handler`
//! concern included into several models.
//!
//! The writer's `_sync_<name>` looked the join model up by the raw
//! `class_name` spelling, missed it (`"::Ns::Labeling"` is not
//! `"Ns::Labeling"`), and fell back to snake-casing the qualified target,
//! emitting `__join.::ns::label_id = __target.id`: a Ruby syntax error
//! that kept the whole model file from loading. It also wrote and
//! scoped the join rows by `<as>_id` alone, leaving `<as>_type` unset.
//! Rails' through writer builds the row through the source `belongs_to`
//! (its `foreign_key:`) and stamps the polymorphic type.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCHEMA_TABLES: &str = "  create_table \"labels\", force: :cascade do |t|\n    t.string \"name\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"labelings\", force: :cascade do |t|\n    t.integer \"labelable_id\", null: false\n    t.string \"labelable_type\", null: false\n    t.integer \"label_key_id\", null: false\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"articles\", force: :cascade do |t|";

#[test]
fn a_through_writer_with_anchored_class_names_writes_the_source_key_and_type() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", "  create_table \"articles\", force: :cascade do |t|", SCHEMA_TABLES)
        .write(
            "app/models/ns/label.rb",
            "module Ns\n  class Label < ApplicationRecord\n    self.table_name = \"labels\"\n  end\nend\n",
        )
        .write(
            "app/models/ns/labeling.rb",
            "module Ns\n  class Labeling < ApplicationRecord\n    self.table_name = \"labelings\"\n    belongs_to :labelable, polymorphic: true\n    belongs_to :label, class_name: \"::Ns::Label\", foreign_key: :label_key_id\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  has_many :labelings, class_name: \"::Ns::Labeling\", as: :labelable\n  has_many :labels, class_name: \"::Ns::Label\", through: :labelings, source: :label\n",
        )
        .run_ruby(
            r#"
Ns::Labeling.delete_all
Article.delete_all
Ns::Label.delete_all
a = Ns::Label.create!(name: "a")
b = Ns::Label.create!(name: "b")
saved = Article.create!(title: "Persisted", body: "Long enough body")
saved.labels = [a, b]
rows = Ns::Labeling.all.to_a
raise "join rows: #{rows.size}, want 2" unless rows.size == 2
raise "label_key_id: #{rows.map(&:label_key_id).sort.inspect}" unless rows.map(&:label_key_id).sort == [a.id, b.id].sort
raise "labelable_type: #{rows.map(&:labelable_type).inspect}" unless rows.all? { |r| r.labelable_type == "Article" }
raise "labelable_id" unless rows.all? { |r| r.labelable_id == saved.id }
saved.labels = [b]
rows = Ns::Labeling.all.to_a
raise "replace: #{rows.size} join rows, want 1" unless rows.size == 1 && rows.first.label_key_id == b.id
puts "anchored through writer ok"
"#,
        );
    run.assert_passes();
    let article = std::fs::read_to_string(run.emitted.join("app/models/article.rb"))
        .expect("emitted article.rb");
    assert!(!article.contains("__join.::"), "emitted a `::`-pathed join writer:\n{article}");
    assert!(article.contains("__join.label_key_id = "), "source belongs_to key not used:\n{article}");
}
