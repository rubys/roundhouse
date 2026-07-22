//! Nested `has_many :through` — the lobsters `Category has_many
//! :stories, through: :tags` shape, where the join model (Tag) reaches
//! the target through ANOTHER association (`Tag has_many :stories,
//! through: :taggings`) rather than a `belongs_to`.
//!
//! Two halves, both surfaced by the lobsters spinel-AOT probe:
//!
//! - WRITER: the through collection writer used to synthesize
//!   `_sync_stories` against the `<target>_id` convention — `Tag.new` +
//!   `__join.story_id = …` on a model with no such column (hard compile
//!   stop under spinel AOT, latent NoMethodError on CRuby). Rails makes
//!   nested through collections read-only
//!   (HasManyThroughNestedAssociationsAreReadonly), so the honest
//!   lowering is NO writer — the skip is ledgered as `lower_residue`.
//! - READER: `apply_through_assoc_lowering` bailed on the nested shape,
//!   leaving the shared direct-fk reader `Story.where(category_id: @id)`
//!   — no such column, silently-wrong SQL at runtime. The chain resolver
//!   now recurses: one INNER JOIN per hop, WHERE on the owner-nearest
//!   edge.
//!
//! The simple shape (`Tag has_many :stories, through: :taggings`,
//! `Tagging.belongs_to :story`) must keep its writer and its
//! single-hop join byte-for-byte.

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
  create_table "taggings", force: :cascade do |t|
    t.integer "story_id", null: false
    t.integer "tag_id", null: false
  end
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
  end
end
"#,
        ),
        (
            "app/models/category.rb",
            r#"class Category < ApplicationRecord
  has_many :tags
  has_many :stories, through: :tags
end
"#,
        ),
        (
            "app/models/tag.rb",
            r#"class Tag < ApplicationRecord
  belongs_to :category
  has_many :taggings
  has_many :stories, through: :taggings
end
"#,
        ),
        (
            "app/models/tagging.rb",
            r#"class Tagging < ApplicationRecord
  belongs_to :tag
  belongs_to :story
end
"#,
        ),
        (
            "app/models/story.rb",
            r#"class Story < ApplicationRecord
  has_many :taggings
  has_many :tags, through: :taggings
end
"#,
        ),
    ]))
    .expect("ingest nested-through app")
}

fn model_src(name: &str) -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(name))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {name}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

#[test]
fn nested_through_gets_no_collection_writer_or_sync() {
    let src = model_src("category.rb");
    // No writer, no join-row sync, no after_save fold, no stale flag —
    // Rails raises HasManyThroughNestedAssociationsAreReadonly on
    // assignment; a missing writer is the honest equivalent.
    assert!(!src.contains("def stories="), "nested through must not get a writer:\n{src}");
    assert!(!src.contains("_sync_stories"), "nested through must not get a sync:\n{src}");
    assert!(!src.contains("@stories_stale"), "no writer, no stale flag:\n{src}");
    // The reader and its preload seam stay.
    assert!(src.contains("def stories"), "{src}");
    assert!(src.contains("def _preload_stories"), "{src}");
}

#[test]
fn nested_through_reader_joins_each_hop() {
    let src = model_src("category.rb");
    // Category → (tags) → taggings → stories: one INNER JOIN per hop,
    // WHERE on the owner-nearest edge (tags.category_id).
    assert!(
        src.contains(
            "INNER JOIN taggings ON taggings.story_id = stories.id \
             INNER JOIN tags ON tags.id = taggings.tag_id"
        ),
        "nested reader must join both hops:\n{src}"
    );
    assert!(src.contains("tags.category_id = ?"), "{src}");
    // The silently-wrong direct-fk fallback (stories has no category_id)
    // must be gone.
    assert!(
        !src.contains("where({ category_id: @id })"),
        "shared direct-fk reader must be replaced:\n{src}"
    );
}

#[test]
fn simple_through_keeps_writer_and_single_hop_join() {
    let src = model_src("tag.rb");
    // Tagging carries both fks (belongs_to :tag / :story) — the writer
    // synthesizes, with the fk resolved from the join model's belongs_to.
    assert!(src.contains("def stories="), "simple through keeps its writer:\n{src}");
    assert!(src.contains("def _sync_stories"), "{src}");
    assert!(src.contains("story_id = __target.id"), "{src}");
    // Reader stays the single-hop join, byte-identical to the
    // pre-resolver shape.
    assert!(
        src.contains("INNER JOIN taggings ON taggings.story_id = stories.id"),
        "{src}"
    );
    assert!(src.contains("taggings.tag_id = ?"), "{src}");
}
