//! Inline chain guard for the Arel materializer
//! (`lower::arel::rewrite_arel_in_expr` — the inline sibling of the
//! refined-names guard).
//!
//! A relation-refiner link `try_build_arel` can't lift (string
//! `order("tag asc")`, a chained `.where`, `references(...)`) must not
//! have the liftable base underneath it claimed and materialized — the
//! refiner would dangle on a hydrated Array (`results.order("tag
//! asc")` — NoMethodError on every lane, hard compile stop under
//! spinel AOT). Surfaced by the lobsters capture: `Category has_many
//! :tags, -> { order('tag asc') }` readers and `Category.all.order(…)…`
//! controller chains. Guarded chains stay whole and run on the runtime
//! `ActiveRecord::Relation`; fully-liftable chains keep materializing.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "categories", force: :cascade do |t|
    t.string "category", null: false
  end
  create_table "tags", force: :cascade do |t|
    t.string "tag", null: false
    t.integer "category_id", null: false
  end
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
    t.integer "score", null: false
  end
end
"#,
        ),
        (
            "app/models/category.rb",
            r#"class Category < ApplicationRecord
  has_many :tags, -> { order('tag asc') }
end
"#,
        ),
        (
            "app/models/tag.rb",
            r#"class Tag < ApplicationRecord
  belongs_to :category
end
"#,
        ),
        (
            "app/models/story.rb",
            r#"class Story < ApplicationRecord
end
"#,
        ),
        (
            "app/controllers/home_controller.rb",
            r#"class HomeController < ApplicationController
  def index
    @categories = Category.all.order("category asc").includes(:tags)
  end

  def sorted
    @stories = Story.order(score: :desc)
  end
end
"#,
        ),
        (
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "/", to: "home#index"
  get "/sorted", to: "home#sorted"
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn emitted(files: &[roundhouse::emit::EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

#[test]
fn scoped_has_many_reader_keeps_the_order_chain_whole() {
    let files = ruby::emit_lowered_models(&app());
    let category = emitted(&files, "app/models/category.rb");
    // The scope's string `order` can't fold into SQL, so the whole
    // reader chain must stay on the runtime Relation…
    assert!(
        category.contains(r#".order("tag asc")"#),
        "reader keeps the scope's order chain:\n{category}"
    );
    // …and the FK query underneath must NOT be claimed out from under
    // it (the dangling-refiner shape: hydrate loop + `results.order`).
    assert!(
        !category.contains("results.order"),
        "no dangling refiner on a hydrated Array:\n{category}"
    );
}

#[test]
fn unliftable_controller_chain_is_not_materialized_under_the_refiner() {
    let files = ruby::emit_lowered_controllers(&app());
    let home = emitted(&files, "app/controllers/home_controller.rb");
    assert!(
        home.contains(r#".order("category asc")"#),
        "string-order chain survives whole:\n{home}"
    );
    assert!(
        !home.contains("results.order") && !home.contains("FROM categories"),
        "`Category.all` must not be claimed under the unlifted chain:\n{home}"
    );
}

#[test]
fn fully_liftable_chain_still_materializes() {
    let files = ruby::emit_lowered_controllers(&app());
    let home = emitted(&files, "app/controllers/home_controller.rb");
    // Control: `Story.order(score: :desc)` lifts end-to-end (implicit
    // `.all` base + symbol order) and keeps compiling to direct SQL.
    assert!(
        home.contains("ORDER BY score DESC"),
        "liftable symbol-order chain still folds to SQL:\n{home}"
    );
}
