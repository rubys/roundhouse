//! `include Discard::Model` (the `discard` gem): admitted when the gem is
//! locked and the table carries the discard column, expanded into the
//! scopes and predicates it stands for; refused otherwise.

use roundhouse::analyze::diagnose;
use roundhouse::diagnostic::Severity;
use roundhouse::dialect::{MethodReceiver, ModelBodyItem};
use roundhouse::ingest::ingest_app_from_tree;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    discard (2.0.0)\n      activerecord (>= 5.0, < 9.0)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  discard (~> 2.0)\n";

fn tree(lock: Option<&str>, schema: &str, models: &[(&str, &str)]) -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
    let mut files: Vec<(String, String)> = vec![("db/schema.rb".into(), schema.into())];
    if let Some(lock) = lock {
        files.push(("Gemfile.lock".into(), lock.into()));
    }
    for (name, src) in models {
        files.push((format!("app/models/{name}.rb"), (*src).into()));
    }
    files.into_iter().map(|(p, c)| (p.into(), c.into_bytes())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :articles do |t|
    t.string :title
    t.datetime :discarded_at
  end
  create_table :comments do |t|
    t.integer :article_id
    t.datetime :removed_at
  end
  create_table :notes do |t|
    t.string :title
  end
end
"#;

const ARTICLE: &str = r#"class Article < ApplicationRecord
  include Discard::Model
  has_many :comments

  def self.listing
    kept.order(:id)
  end

  def self.everything
    with_discarded
  end

  def self.hidden
    discarded
  end

  def comment_count
    comments.kept.count
  end

  def all_comments
    comments.with_discarded
  end

  def gone?
    discarded?
  end

  def present?
    kept?
  end
end
"#;

const COMMENT: &str = "class Comment < ApplicationRecord\n  include Discard::Model\n  self.discard_column = :removed_at\n  belongs_to :article\nend\n";

fn errors(app: &mut roundhouse::App) -> Vec<String> {
    let lower = roundhouse::session::analyze_and_lower(app);
    diagnose(app)
        .into_iter()
        .chain(lower)
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message)
        .collect()
}

#[test]
fn include_is_admitted_and_scopes_and_predicates_resolve() {
    let mut app = ingest_app_from_tree(tree(Some(LOCK), SCHEMA, &[("article", ARTICLE), ("comment", COMMENT)]))
        .expect("ingest");
    let errs = errors(&mut app);
    assert!(errs.is_empty(), "{errs:?}");

    let article = app.models.iter().find(|m| m.name.0.as_str() == "Article").unwrap();
    let scopes: Vec<&str> = article.body.iter().filter_map(|i| match i {
        ModelBodyItem::Scope { scope, .. } => Some(scope.name.as_str()),
        _ => None,
    }).collect();
    assert_eq!(scopes, ["kept", "undiscarded", "discarded", "with_discarded"]);
    // The include is the expansion, not an extra unknown statement.
    assert!(!article.body.iter().any(|i| matches!(i, ModelBodyItem::Unknown { .. })));

    // Types: a kept chain is a relation of Article; the predicates are Bool.
    let ret = |name: &str, recv: MethodReceiver| {
        let m = article.body.iter().find_map(|i| match i {
            ModelBodyItem::Method { method, .. } if method.name.as_str() == name && method.receiver == recv => Some(method),
            _ => None,
        }).unwrap_or_else(|| panic!("{name}"));
        format!("{:?}", m.body.ty)
    };
    for name in ["listing", "everything", "hidden"] {
        let ty = ret(name, MethodReceiver::Class);
        assert!(ty.contains("Relation") && ty.contains("Article"), "{name}: {ty}");
    }
    assert!(ret("gone?", MethodReceiver::Instance).contains("Bool"));
    assert!(ret("present?", MethodReceiver::Instance).contains("Bool"));
}

#[test]
fn custom_discard_column_is_honored() {
    let app = ingest_app_from_tree(tree(Some(LOCK), SCHEMA, &[("article", ARTICLE), ("comment", COMMENT)])).unwrap();
    let comment = app.models.iter().find(|m| m.name.0.as_str() == "Comment").unwrap();
    let kept = comment.body.iter().find_map(|i| match i {
        ModelBodyItem::Scope { scope, .. } if scope.name.as_str() == "kept" => Some(format!("{:?}", scope.body)),
        _ => None,
    }).unwrap();
    assert!(kept.contains("removed_at") && !kept.contains("discarded_at"), "{kept}");
    // The assignment was consumed, not left to run as an unknown call.
    assert!(!comment.body.iter().any(|i| matches!(i, ModelBodyItem::Unknown { .. })));
}

#[test]
fn a_table_without_the_column_is_refused() {
    let note = "class Note < ApplicationRecord\n  include Discard::Model\nend\n";
    let err = ingest_app_from_tree(tree(Some(LOCK), SCHEMA, &[("note", note)])).err().expect("refused");
    let msg = err.to_string();
    assert!(msg.contains("Discard::Model") && msg.contains("notes") && msg.contains("discarded_at"), "{msg}");
}

#[test]
fn a_non_literal_discard_column_is_refused() {
    let src = "class Article < ApplicationRecord\n  include Discard::Model\n  self.discard_column = COLUMN\nend\n";
    let err = ingest_app_from_tree(tree(Some(LOCK), SCHEMA, &[("article", src)])).err().expect("refused");
    assert!(err.to_string().contains("literal symbol"), "{err}");
}

#[test]
fn without_the_gem_in_the_lock_the_include_stays_unresolved() {
    let mut app = ingest_app_from_tree(tree(None, SCHEMA, &[("article", ARTICLE)])).expect("ingest");
    let errs = errors(&mut app);
    assert!(errs.iter().any(|e| e.contains("includes unresolved Discard::Model")), "{errs:?}");
}

#[test]
fn a_before_discard_hook_is_refused_not_dropped() {
    let src = "class Article < ApplicationRecord\n  include Discard::Model\n  before_discard :check\n\n  def check\n    true\n  end\nend\n";
    let err = ingest_app_from_tree(tree(Some(LOCK), SCHEMA, &[("article", src)])).err().expect("refused");
    assert!(err.to_string().contains("before_discard"), "{err}");
}
