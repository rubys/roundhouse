//! Runtime Relations can arise without a named scope, for example when a
//! helper returns a where.not query that cannot be folded into static SQL.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Build a scope-free app whose helper returns a runtime Relation.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "authors", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "books", force: :cascade do |t|
    t.integer "author_id"
  end
end
"#)
        .write("app/models/author.rb", "class Author < ApplicationRecord\nend\n")
        .write("app/models/book.rb", "class Book < ApplicationRecord\n  belongs_to :author, optional: true\nend\n")
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
  author = Author.create!(name: "author#{i}")
  Book.create!(author_id: author.id)
end
controller = BooksController.new
sql = Db.capture_sql { controller.process_action(:index) }
raise controller.body unless controller.body == "author0,author1,author2,author3"
raise "expected 2 queries, got #{sql.length}: #{sql.join("; ")}" unless sql.length == 2
puts "scope-free preloading passed"
"#;

/// A runtime includes hint must load all assigned authors in one batch.
#[test]
fn runtime_includes_without_named_scopes_batches_belongs_to() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

/// An absent author remains nil without adding a per-book query on CRuby.
#[test]
fn nullable_belongs_to_preloading_runs_on_ruby() {
    let script = ASSERTIONS
        .replace("controller = BooksController.new", "Book.create!(author_id: nil)\ncontroller = BooksController.new")
        .replace("author2,author3", "author2,author3,none");
    app().run_ruby(&script).assert_passes();
}

/// The compiled app must preserve the same values and two-query bound.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn runtime_includes_without_named_scopes_batches_belongs_to_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{ASSERTIONS}"
    );
    app().run_spinel(&script).assert_passes();
}

/// Build a runtime collection query with an association restriction.
fn collection_app(association: &str) -> emit_and_run::Overlay {
    app()
        .write("app/models/book.rb", "class Book < ApplicationRecord\nend\n")
        .write("app/models/author.rb", &format!("class Author < ApplicationRecord\n  {association}\nend\n"))
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "authors", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "books", force: :cascade do |t|
    t.integer "author_id"
    t.integer "owner_id"
    t.string "owner_type"
    t.integer "visible"
    t.string "title"
  end
end
"#)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/authors\", to: \"authors#index\"\nend\n")
        .write("app/controllers/authors_controller.rb", r#"class AuthorsController < ApplicationController
  def index
    render plain: authors_for_list.map { |author| author.books.map { |book| book.title }.join(",") }.join(";")
  end

  private

  def authors_for_list
    Author.all.where.not(id: 0).order(:id).includes(:books)
  end
end
"#)
        .remove("app/controllers/books_controller.rb")
}

const PLAIN_COLLECTION_ASSERTIONS: &str = r#"
require_relative "app/controllers/authors_controller"
4.times do |i|
  author = Author.create!(name: "writer#{i}")
  Book.create!(author_id: author.id, title: "book#{i}")
end
controller = AuthorsController.new
sql = Db.capture_sql { controller.process_action(:index) }
raise controller.body unless controller.body == "book0;book1;book2;book3"
raise "expected 2 queries, got #{sql.length}" unless sql.length == 2
"#;

const SCOPED_COLLECTION_ASSERTIONS: &str = r#"
require_relative "app/controllers/authors_controller"
author = Author.create!(name: "writer")
Book.create!(author_id: author.id, visible: 1, title: "visible")
Book.create!(author_id: author.id, visible: 0, title: "hidden")
controller = AuthorsController.new
controller.process_action(:index)
raise controller.body unless controller.body == "visible"
"#;

const POLYMORPHIC_COLLECTION_ASSERTIONS: &str = r#"
require_relative "app/controllers/authors_controller"
author = Author.create!(name: "writer")
Book.create!(owner_id: author.id, owner_type: "Author", title: "owned")
Book.create!(owner_id: author.id, owner_type: "Publisher", title: "other-owner")
controller = AuthorsController.new
controller.process_action(:index)
raise controller.body unless controller.body == "owned"
"#;

/// Unrestricted collection associations still batch after restricted ones fall back.
#[test]
fn plain_has_many_still_batches() {
    collection_app("has_many :books")
        .run_ruby(PLAIN_COLLECTION_ASSERTIONS).assert_passes();
}

/// Enabling runtime preloading must not cache rows excluded by a scope.
#[test]
fn scoped_has_many_keeps_its_filter() {
    collection_app("has_many :books, -> { where(visible: 1) }")
        .run_ruby(SCOPED_COLLECTION_ASSERTIONS).assert_passes();
}

/// Equal IDs from different owner types must not share preloaded rows.
#[test]
fn polymorphic_has_many_keeps_its_owner_type() {
    collection_app("has_many :books, as: :owner")
        .run_ruby(POLYMORPHIC_COLLECTION_ASSERTIONS).assert_passes();
}

/// Boot the emitted in-memory SQLite database before a compiled assertion script.
fn native_script(assertions: &str) -> String {
    format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{assertions}"
    )
}

/// The native app retains the two-query batch path for unrestricted collections.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn plain_has_many_still_batches_on_spinel() {
    collection_app("has_many :books")
        .run_spinel(&native_script(PLAIN_COLLECTION_ASSERTIONS)).assert_passes();
}

/// The compiled collection reader preserves the association's visibility filter.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn scoped_has_many_keeps_its_filter_on_spinel() {
    collection_app("has_many :books, -> { where(visible: 1) }")
        .run_spinel(&native_script(SCOPED_COLLECTION_ASSERTIONS)).assert_passes();
}

/// The compiled collection reader preserves the polymorphic owner-type filter.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn polymorphic_has_many_keeps_its_owner_type_on_spinel() {
    collection_app("has_many :books, as: :owner")
        .run_spinel(&native_script(POLYMORPHIC_COLLECTION_ASSERTIONS)).assert_passes();
}
