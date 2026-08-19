//! `<relation>.select { … }` is Enumerable, not the projection.
//!
//! Rails puts two methods on the name `select`: with column specs it
//! projects (`select(:id)` → `SELECT tags.id`), with a block it filters
//! the loaded rows. The runtime keeps them apart — `select(*specs)` and
//! `filter` — and `lower::relation_select_block` routes the block form
//! to the second so the first can answer a Relation and nothing else.
//!
//! The reason that matters is not tidiness. A `select` answering
//! `Relation | Array` types every receiver below it POLY, and poly is a
//! different DISPATCH PATH on the strict targets, not just a slower one:
//! spinel's does not convert braceless keyword args into the trailing
//! positional Hash the callee's optional parameter expects, so
//! `Story.select(:id).where(merged_story_id: id)` silently bound
//! `where`'s `condition` to `nil` and lost the filter — lobsters'
//! `/s/:story_id` answered with every merged story's comments.
//!
//! The gate is the receiver's analyzer type: `Ty::Relation`, or no
//! type at all (campfire filters a relation passed in as a method
//! PARAMETER, which types untyped — and `filter` is an exact Ruby alias
//! of `select` on everything else such a receiver could be, so guessing
//! wrong there costs nothing). A receiver typed as something concrete
//! and non-relational — a plain Array — is left alone.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

fn tag_model() -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "tags", force: :cascade do |t|
    t.string "tag", null: false
    t.boolean "active", default: true, null: false
  end
end
"#,
        ),
        (
            "app/models/tag.rb",
            r#"class Tag < ApplicationRecord
  scope :active, -> { where(active: true) }

  def self.pickable(user)
    Tag.active.order(:tag).select { |t| t.valid_for?(user) }
  end

  def self.projection
    Tag.active.select(:id)
  end

  def self.short
    ["a", "bb", "ccc"].select { |n| n.length > 2 }
  end

  # campfire's shape: the relation is a PARAMETER, so the analyzer has
  # no type for it and the narrow gate would miss the site.
  def self.pickable_via_param(rel, user)
    rel.select { |t| t.valid_for?(user) }
  end

  def valid_for?(user)
    user != tag
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    files
        .iter()
        .find(|f| f.path.ends_with("tag.rb"))
        .unwrap_or_else(|| panic!("no tag.rb in {:?}", files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()))
        .content
        .clone()
}

#[test]
fn relation_typed_select_block_becomes_filter() {
    let src = tag_model();
    let line = src
        .lines()
        .find(|l| l.contains("valid_for?(user) }"))
        .unwrap_or_else(|| panic!("no pickable body in:\n{src}"));
    assert!(
        line.contains(".filter {"),
        "relation-typed `select {{ … }}` should lower to `filter`, got:\n{line}"
    );
    assert!(
        !line.contains(".select {"),
        "the block form should not survive on `select`, got:\n{line}"
    );
}

#[test]
fn array_typed_select_block_is_left_alone() {
    let src = tag_model();
    let line = src
        .lines()
        .find(|l| l.contains("n.length > 2"))
        .unwrap_or_else(|| panic!("no short body in:\n{src}"));
    assert!(
        line.contains(".select {"),
        "Enumerable's own `select` on an Array is already the right method, got:\n{line}"
    );
}

#[test]
fn column_projection_keeps_the_name() {
    let src = tag_model();
    let line = src
        .lines()
        .find(|l| l.contains("def self.projection") || l.contains("select(:id)"))
        .unwrap_or_else(|| panic!("no projection body in:\n{src}"));
    assert!(
        !line.contains("filter"),
        "`select(:id)` is the projection and must keep its name, got:\n{line}"
    );
}

#[test]
fn untyped_receiver_is_rewritten_too() {
    let src = tag_model();
    let line = src
        .lines()
        .find(|l| l.contains("valid_for?(user) }") && l.contains("rel"))
        .unwrap_or_else(|| panic!("no pickable_via_param body in:\n{src}"));
    assert!(
        line.contains(".filter {"),
        "a relation arriving as an untyped parameter must still reach `filter`, got:\n{line}"
    );
}
