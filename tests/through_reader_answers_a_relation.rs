//! A `has_many :through` reader answers a Relation on EVERY path.
//!
//! The shared has_many lowering opens a reader with `return
//! @<name>_cache if @<name>_loaded` — the eager-load seam, and the
//! right answer for a direct has_many, whose reader materializes rows
//! and is declared `Array[T]`. A `through:` reader is declared
//! `ActiveRecord::Relation` (the join lives on the intermediate table;
//! see `lower::model_to_library::associations` and the campfire routing
//! bug its comment names), and that guard made one method answer two
//! unrelated types:
//!
//!   - on CRuby, latent: a preloaded `user.upvoted_stories
//!     .includes(:tags).order(...)` reaches `Array#includes`, which
//!     does not exist;
//!   - under spinel's AOT, a hard compile stop — `--rbs seed
//!     contradicted: User#upvoted_stories is declared to return
//!     Relation but this returns int_array` — which took the lobsters
//!     bench lane's AOT row down for three days. (`int_array` because
//!     nothing in that app preloads the association, so the cache ivar
//!     types off the bare `[]` in `initialize`.)
//!
//! The preload seam moves onto the relation: `_preload_<name>` still
//! fills the cache ivar, and the reader hands it to
//! `Relation#preloaded`, which seeds the loaded-records memo when the
//! flag is set and is a no-op when it is not.

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

/// The lobsters shape that broke: `User has_many :upvoted_stories,
/// through: :votes, source: :story` with an association scope, plus a
/// plain `has_many :stories` to hold the direct-reader shape still.
fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "username", null: false
  end
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
    t.integer "user_id", null: false
  end
  create_table "votes", force: :cascade do |t|
    t.integer "user_id", null: false
    t.integer "story_id", null: false
    t.integer "vote", null: false
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_many :stories
  has_many :votes
  has_many :upvoted_stories, -> { where("votes.vote" => 1) }, through: :votes, source: :story
end
"#,
        ),
        (
            "app/models/story.rb",
            r#"class Story < ApplicationRecord
  belongs_to :user
  has_many :votes
end
"#,
        ),
        (
            "app/models/vote.rb",
            r#"class Vote < ApplicationRecord
  belongs_to :user
  belongs_to :story
end
"#,
        ),
    ]))
    .expect("ingest through-with-scope app")
}

fn emitted(name: &str) -> String {
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

/// The body of `def <name>` up to its `end`, so an assertion about one
/// reader can't be satisfied by text belonging to another method.
fn method_body(src: &str, name: &str) -> String {
    let head = format!("  def {name}\n");
    let start = src.find(&head).unwrap_or_else(|| panic!("no `def {name}` in:\n{src}"));
    let rest = &src[start + head.len()..];
    let end = rest.find("\n  end\n").unwrap_or_else(|| panic!("unterminated `def {name}`"));
    rest[..end].to_string()
}

#[test]
fn the_through_reader_has_no_array_returning_guard() {
    let body = method_body(&emitted("user.rb"), "upvoted_stories");
    assert!(
        !body.contains("return @upvoted_stories_cache"),
        "the cache guard answers an Array from a Relation-typed reader:\n{body}"
    );
    // One statement, and it is the relation.
    assert!(body.trim().starts_with("ActiveRecord::Relation.new(Story)"), "{body}");
    assert_eq!(body.trim().lines().count(), 1, "one return path:\n{body}");
}

#[test]
fn the_cache_reaches_the_relation_instead() {
    let src = emitted("user.rb");
    let body = method_body(&src, "upvoted_stories");
    assert!(
        body.contains(".preloaded(@upvoted_stories_cache, @upvoted_stories_loaded)"),
        "the eager-load cache must reach the relation:\n{body}"
    );
    // The seam that fills it is untouched — `includes(:upvoted_stories)`
    // still preloads through the generated writer.
    assert!(src.contains("def _preload_upvoted_stories"), "{src}");
    assert!(src.contains("@upvoted_stories_cache = list"), "{src}");
}

#[test]
fn the_association_scope_lands_on_the_query_not_after_the_cache() {
    let body = method_body(&emitted("user.rb"), "upvoted_stories");
    let scope = body.find(r#".where("votes.vote" => 1)"#).expect(&format!("scope graft:\n{body}"));
    let preloaded = body.find(".preloaded(").expect(&format!("preloaded:\n{body}"));
    // `preloaded` is LAST: the scope's conditions belong to the query it
    // would run, not to the records an eager load already fetched.
    assert!(scope < preloaded, "the scope must graft onto the query:\n{body}");
}

#[test]
fn a_direct_has_many_keeps_its_array_guard() {
    // Unchanged: that reader materializes rows and is declared
    // `Array[Story]`, so the cache guard agrees with it.
    let body = method_body(&emitted("user.rb"), "stories");
    assert!(body.contains("return @stories_cache if @stories_loaded"), "{body}");
    assert!(!body.contains("preloaded("), "{body}");
}

#[test]
fn the_runtime_relation_answers_preloaded() {
    // The emitted call is only as good as the runtime behind it: the
    // ruby-family Relation must define `preloaded`, and its sidecar must
    // declare it, or every through reader is a NoMethodError on CRuby
    // and an unresolved send under a strict typer.
    let rb = std::fs::read_to_string("runtime/ruby/active_record/relation.rb")
        .expect("runtime relation.rb");
    let rbs = std::fs::read_to_string("runtime/ruby/active_record/relation.rbs")
        .expect("runtime relation.rbs");
    assert!(rb.contains("def preloaded(records, loaded)"), "{rb}");
    // `records` is `untyped` ON PURPOSE — do not narrow it back to
    // `Array[untyped]`. That is a REPRESENTATION claim, not "any
    // array", and `--rbs` seeds are trusted: a caller holding an
    // `Array[Integer]` (campfire's `@reachable_messages_cache`, seeded
    // `[]` in the constructor) contradicts it and the build stops.
    // This method only stores the argument, so it promises nothing
    // about the array's shape.
    assert!(rbs.contains("def preloaded: (untyped records, bool loaded) -> Relation"));
    // Chain methods drop the memo, so narrowing a preloaded relation
    // re-queries rather than filtering a stale set.
    assert!(rb.contains("@records = records if loaded"), "{rb}");
}
