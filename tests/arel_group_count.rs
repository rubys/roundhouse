//! `group(:col).count` — Rails' grouped count — folds into the SELECT
//! and hydrates a Hash, on every target (issues #77 and #78).
//!
//! Three things had to be true at once for a grouped count to reach any
//! target, and none of them were:
//!
//! 1. `lower::group_count` renames the terminal to `group_count` (so the
//!    scalar `count` keeps its Integer return) and used to CLEAR the
//!    type. `analyze::diagnostics` reads `expr.ty` as it stands — it
//!    does not re-dispatch — so a typeless Send with a known receiver
//!    read as `send_dispatch_failed: no known method group_count on
//!    Relation[Comment]`, against a name the app never wrote, a runtime
//!    method that exists, and an RBS signature that declares it. Every
//!    target failed the type gate on a feature the pipeline ships.
//! 2. The Arel builder had no `group` arm, so no grouped chain folded.
//!    On the ruby-family targets that is a missed specialization; on the
//!    relation-less ones (rust, go, crystal, typescript, python) the
//!    fold is the ONLY path — they have no Relation to execute a chain
//!    on.
//! 3. The visitor had no Hash-yielding hydrate. Every result shape it
//!    emitted was Array-of-model, Array-of-column, a single record, or
//!    a scalar.
//!
//! The fold is licensed by the TERMINAL, never by `group` alone: a bare
//! `group(:col)` would render `SELECT * … GROUP BY col`, one arbitrary
//! row per group, which is not what any chain carrying it means. So the
//! builder recognizes the `group(:col).count` PAIR and declines
//! everything else, leaving it on the runtime Relation.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::session::analyze_and_lower;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// Written as MODEL class methods on purpose: the typer demotes a
/// trailing kwargs hash to positional when the callee's signature
/// declares a positional Hash — which `Base.self.where` does — and
/// controllers re-type immediately BEFORE the arel pass while models
/// re-type after, so the identical `where` chain lifts in a model and
/// does not in a controller. Same reason `tests/arel_pluck.rs` is
/// written where it is; the `where`-less cases here are unaffected
/// either way.
fn app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.text "body", null: false
    t.string "commenter"
    t.integer "article_id", null: false
  end
end
"#,
        ),
        (
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  belongs_to :article
  scope :newest, -> { order("id desc") }

  def self.by_article
    Comment.group(:article_id).count
  end

  def self.by_commenter
    Comment.group(:commenter).count
  end

  def self.filtered
    Comment.where(commenter: "x").group(:article_id).count
  end

  def self.two_groups
    Comment.group(:article_id, :commenter).count
  end

  def self.unknown_column
    Comment.group(:nonexistent).count
  end

  def self.limited
    Comment.limit(5).group(:article_id).count
  end

  def self.grouped_rows
    Comment.group(:article_id).to_a
  end

  def self.scope_rooted
    Comment.newest.group(:article_id).count
  end
end
"#,
        ),
        (
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/\", to: \"home#index\"\nend\n",
        ),
    ]))
    .expect("ingest");
    analyze_and_lower(&mut app);
    app
}

fn comment_model() -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/models/comment.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("no emitted comment model"))
}

/// One emitted class method's body, so a per-shape assertion can't be
/// satisfied by a sibling method's emit.
fn method_body(src: &str, name: &str) -> String {
    let head = format!("  def self.{name}\n");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("no emitted `def self.{name}` in:\n{src}"))
        + head.len();
    let rest = &src[start..];
    let end = rest.find("\n  end").unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn a_grouped_count_folds_to_group_by_sql() {
    let body = method_body(&comment_model(), "by_article");
    assert!(
        body.contains("SELECT article_id, COUNT(*) FROM comments GROUP BY article_id"),
        "the grouped column is projected beside the aggregate and named \
         in the GROUP BY:\n{body}"
    );
    assert!(
        !body.contains("group_count"),
        "nothing is left for the runtime Relation to answer:\n{body}"
    );
}

#[test]
fn the_hydrate_builds_a_hash_keyed_by_the_group_column() {
    let body = method_body(&comment_model(), "by_article");
    assert!(
        body.contains("results = {}"),
        "the accumulator is a Hash, the shape Rails' grouped count \
         answers with:\n{body}"
    );
    assert!(
        body.contains("results[Db.column_int(stmt, 0)] = Db.column_int(stmt, 1)"),
        "key at index 0, COUNT(*) at index 1 — the projection order:\n{body}"
    );
}

#[test]
fn the_key_read_comes_from_the_column_type() {
    // `commenter` is a nullable string column, so the key reads through
    // the schema-driven reader (the same one the row hydrate and
    // `pluck` use — where the nullable `_opt` rule lives), not through
    // a second copy of that rule that guesses Int.
    let body = method_body(&comment_model(), "by_commenter");
    assert!(
        body.contains("results[Db.column_text_opt(stmt, 0)] = Db.column_int(stmt, 1)"),
        "the group key is read as the column's own type; the count is \
         always an Integer:\n{body}"
    );
}

#[test]
fn a_where_survives_into_the_grouped_select() {
    let body = method_body(&comment_model(), "filtered");
    assert!(
        body.contains(
            "SELECT article_id, COUNT(*) FROM comments WHERE commenter = 'x' \
             GROUP BY article_id"
        ),
        "GROUP BY sits after WHERE, and the predicate is not lost:\n{body}"
    );
}

#[test]
fn a_multi_column_group_stays_on_the_relation_path() {
    // Rails answers Array keys for a multi-column group, and the
    // runtime's own `group_count` joins its groups into one key
    // expression and hydrates a flat Hash. Neither side can carry the
    // composite today, so decline rather than guess.
    let body = method_body(&comment_model(), "two_groups");
    assert!(
        body.contains("group_count"),
        "the chain is left whole for the runtime:\n{body}"
    );
    assert!(
        !body.contains("GROUP BY"),
        "no SQL is composed for a group shape we cannot render:\n{body}"
    );
}

#[test]
fn a_column_the_table_lacks_is_declined() {
    let body = method_body(&comment_model(), "unknown_column");
    assert!(
        !body.contains("GROUP BY"),
        "a group of an unknown column must not compose SQL that would \
         fail at run time:\n{body}"
    );
}

#[test]
fn a_limited_chain_is_declined() {
    // Rails' `limit(5).group(:c).count` limits the GROUPS; the SQL
    // composed here expresses no such thing, so folding it would answer
    // a different question. Same rule `apply_count` applies.
    let body = method_body(&comment_model(), "limited");
    assert!(
        !body.contains("GROUP BY"),
        "a limited grouped count stays dynamic rather than answering \
         the unlimited total:\n{body}"
    );
}

#[test]
fn a_bare_group_never_folds_on_its_own() {
    // The fold is licensed by the terminal. A `group` with no grouped
    // terminal under it would render `SELECT * … GROUP BY col` — one
    // arbitrary row per group — so it stays on the runtime Relation.
    let body = method_body(&comment_model(), "grouped_rows");
    assert!(
        !body.contains("GROUP BY"),
        "a bare `group` composes no SQL:\n{body}"
    );
    assert!(
        body.contains(".group(:article_id)"),
        "the chain is left whole:\n{body}"
    );
}

#[test]
fn an_unfoldable_grouped_count_reaches_the_ruby_runtime_method() {
    // Issue #77: the rename is what makes the runtime's
    // `Relation#group_count` reachable. A scope-rooted chain has no
    // Arel base — `try_build_arel` doesn't recognize a scope root — so
    // this is the shape that must still land on the runtime method
    // rather than on a `count` that answers an Integer.
    let body = method_body(&comment_model(), "scope_rooted");
    assert!(
        body.contains("group(:article_id).group_count"),
        "the grouped terminal keeps its own name on the runtime path:\n{body}"
    );
}

#[test]
fn the_renamed_terminal_carries_a_type() {
    // Issue #77's actual failure: `group_count` with `expr.ty = None`
    // is reported as a dispatch failure on a name the app never wrote.
    // The lowering owns the renamed call's type — the body-typer's
    // Hash when it has one (keyed by the grouped column), else
    // `relation.rbs`'s declared `Hash[untyped, Integer]`.
    let app = app();
    let failures: Vec<String> = roundhouse::analyze::diagnose(&app)
        .iter()
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .filter(|d| d.contains("group_count"))
        .collect();
    assert!(
        failures.is_empty(),
        "a grouped count must not fail the type gate: {failures:?}"
    );
}
