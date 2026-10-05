//! A through association must preserve the intermediate polymorphic owner type.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Articles and publishers may have identical IDs without sharing taggings.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "tags", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "taggings", force: :cascade do |t|
    t.integer "taggable_id", null: false
    t.string "taggable_type", null: false
    t.integer "tag_id", null: false
  end
end
"#)
        .write("app/models/article.rb", r#"class Article < ApplicationRecord
  has_many :taggings, as: :taggable
  has_many :tags, through: :taggings

  def self.for_list
    all.where.not(id: 0).order(:id).includes(:tags)
  end
end
"#)
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n")
        .write("app/models/tagging.rb", "class Tagging < ApplicationRecord\n  belongs_to :tag\nend\n")
}

const SEED: &str = r#"
first = Article.create!(title: "first")
second = Article.create!(title: "second")
empty = Article.create!(title: "empty")
owned = Tag.create!(name: "owned")
other = Tag.create!(name: "other-owner")
second_tag = Tag.create!(name: "second")
Tagging.create!(taggable_id: first.id, taggable_type: "Article", tag_id: owned.id)
Tagging.create!(taggable_id: first.id, taggable_type: "Publisher", tag_id: other.id)
Tagging.create!(taggable_id: second.id, taggable_type: "Article", tag_id: second_tag.id)
Tagging.create!(taggable_id: empty.id, taggable_type: "Publisher", tag_id: other.id)
"#;

const LAZY_ASSERTIONS: &str = r#"
actual = [first, second, empty].map { |article| article.tags.map { |tag| tag.name }.join(",") }
raise actual.inspect unless actual == ["owned", "second", ""]
"#;

const PRELOADED_ASSERTIONS: &str = r#"
actual = []
sql = Db.capture_sql do
  actual = Article.for_list.map { |article| article.tags.map { |tag| tag.name }.join(",") }
end
raise actual.inspect unless actual == ["owned", "second", ""]
raise "expected two batch queries: #{sql.inspect}" unless sql.length == 2
"#;

/// Lazy through reads reject another owner's equal ID and preserve empty results.
#[test]
fn lazy_through_keeps_the_polymorphic_owner_type() {
    app().run_ruby(&format!("{SEED}\n{LAZY_ASSERTIONS}")).assert_passes();
}

/// Batched through reads must apply the same owner restriction to every record.
#[test]
fn preloaded_through_keeps_the_polymorphic_owner_type() {
    app().run_ruby(&format!("{SEED}\n{PRELOADED_ASSERTIONS}")).assert_passes();
}

/// An association filter still applies when the reader cannot use a batch loader.
#[test]
fn scoped_through_keeps_both_its_filter_and_owner_type() {
    let filtered = app().write("app/models/article.rb", r#"class Article < ApplicationRecord
  has_many :taggings, as: :taggable
  has_many :tags, -> { where(name: "owned") }, through: :taggings

  def self.for_list
    all.where.not(id: 0).order(:id).includes(:tags)
  end
end
"#);
    let assertions = r#"
Tagging.create!(taggable_id: first.id, taggable_type: "Publisher", tag_id: owned.id)
Tagging.create!(taggable_id: empty.id, taggable_type: "Publisher", tag_id: owned.id)
actual = Article.for_list.map { |article| article.tags.map { |tag| tag.name }.join(",") }
raise actual.inspect unless actual == ["owned", "", ""]
"#;
    filtered.run_ruby(&format!("{SEED}\n{assertions}")).assert_passes();
}

/// Ordering a batched association must not bring back another owner's rows.
#[test]
fn ordered_through_keeps_its_order_and_owner_type() {
    let ordered = app().write("app/models/article.rb", r#"class Article < ApplicationRecord
  has_many :taggings, as: :taggable
  has_many :tags, -> { order("tags.name DESC") }, through: :taggings

  def self.for_list
    all.where.not(id: 0).order(:id).includes(:tags)
  end
end
"#);
    let assertions = r#"
another = Tag.create!(name: "zeta")
Tagging.create!(taggable_id: first.id, taggable_type: "Article", tag_id: another.id)
actual = []
sql = Db.capture_sql do
  actual = Article.for_list.map { |article| article.tags.map { |tag| tag.name }.join(",") }
end
raise actual.inspect unless actual == ["zeta,owned", "second", ""]
raise "expected two batch queries: #{sql.inspect}" unless sql.length == 2
"#;
    ordered.run_ruby(&format!("{SEED}\n{assertions}")).assert_passes();
}

/// Boot the same in-memory schema before running a compiled assertion script.
fn native_script(assertions: &str) -> String {
    format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{SEED}\n{assertions}")
}

/// Native lazy reads retain the same owner-type boundary as Rails.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn lazy_through_keeps_the_polymorphic_owner_type_on_spinel() {
    app().run_spinel(&native_script(LAZY_ASSERTIONS)).assert_passes();
}

/// Native preloading retains the exact rows and two-query batch bound.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn preloaded_through_keeps_the_polymorphic_owner_type_on_spinel() {
    app().run_spinel(&native_script(PRELOADED_ASSERTIONS)).assert_passes();
}
