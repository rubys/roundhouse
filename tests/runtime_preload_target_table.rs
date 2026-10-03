//! A belongs_to batch must use the target model's resolved table, including
//! an explicit table override on a namespaced model.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Isolate table resolution with a namespaced target and an explicit table override.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "writers", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "books", force: :cascade do |t|
    t.integer "author_id"
  end
end
"#)
        .write("app/models/catalog/author.rb", "class Catalog::Author < ApplicationRecord\n  self.table_name = \"writers\"\nend\n")
        .write("app/models/book.rb", "class Book < ApplicationRecord\n  belongs_to :author, class_name: \"Catalog::Author\", optional: true\n  scope :ordered, -> { order(:id) }\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/books\", to: \"books#index\"\nend\n")
        .write("app/controllers/books_controller.rb", r#"class BooksController < ApplicationController
  def index
    render plain: books_for_list.map { |book| book.author ? book.author.name : "none" }.join(",")
  end

  private

  def books_for_list
    Book.all.where.not(id: 0).order(:id).includes(:author)
  end
end
"#)
}

const ASSERTIONS: &str = r#"
require_relative "app/controllers/books_controller"
4.times do |i|
  author = Catalog::Author.create!(name: "author#{i}")
  Book.create!(author_id: author.id)
end
controller = BooksController.new
sql = Db.capture_sql { controller.process_action(:index) }
raise controller.body unless controller.body == "author0,author1,author2,author3"
raise "expected 2 queries, got #{sql.length}: #{sql.join("; ")}" unless sql.length == 2
puts "resolved table preloading passed"
"#;

/// CRuby preloads the resolved target table in one batch.
#[test]
fn belongs_to_preloader_uses_the_resolved_target_table() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

/// Missing authors remain nil while CRuby still uses the resolved batch table.
#[test]
fn nullable_belongs_to_preloading_runs_on_ruby() {
    let script = ASSERTIONS
        .replace("controller = BooksController.new", "Book.create!(author_id: nil)\ncontroller = BooksController.new")
        .replace("author2,author3", "author2,author3,none");
    app().run_ruby(&script).assert_passes();
}

/// The compiled app preserves association values and the two-query bound.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn belongs_to_preloader_uses_the_resolved_target_table_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    app().run_spinel(&script).assert_passes();
}
