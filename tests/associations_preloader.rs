//! `ActiveRecord::Associations::Preloader.new(records:, associations:).call`
//! checks clean when its value is discarded and its records are one
//! model's (it lowers to that model's `preload_associations`) or several
//! models' (`ActiveRecord::Preloading.records`), and keeps its refusal for any other
//! option. `scope:` stays a `Preloader` call; MRI raises unknown-keyword.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn constant_errors(body: &str) -> Vec<String> {
    constant_errors_with(body, &[])
}

fn constant_errors_with(body: &str, extra: &[(&str, &str)]) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    for (path, src) in extra {
        tree.insert(PathBuf::from(path), src.as_bytes().to_vec());
    }
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"title\"\n  end\n  create_table \"comments\" do |t|\n    t.integer \"article_id\"\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/application_record.rb"), b"class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/article.rb"), b"class Article < ApplicationRecord\n  has_many :comments\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/comment.rb"), b"class Comment < ApplicationRecord\n  belongs_to :article\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/digest.rb"), format!("class Digest\n  def self.run\n{body}  end\nend\n").into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .map(|d| d.message)
        .filter(|m| m.contains("Preloader"))
        .collect()
}

#[test]
fn one_models_records_check_clean() {
    let errors = constant_errors(
        "    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n    ActiveRecord::Associations::Preloader.new(records: Article.all, associations: [:comments]).call\n    nil\n",
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

/// `call` answers nil, including when the value is used. The class
/// exists, so neither call is an unsupported-constant error.
#[test]
fn a_used_value_checks_clean() {
    let errors = constant_errors(
        "    if ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n      1\n    end\n    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n",
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

/// Several models' records, or records of a type analysis does not know, are grouped by class at run time (`ActiveRecord::Preloading.records`), as Rails' preloader groups them.
#[test]
fn mixed_or_unknown_records_check_clean() {
    let errors = constant_errors(
        "    ActiveRecord::Associations::Preloader.new(records: [Comment.first, Article.first], associations: :article).call\n    nil\n",
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

/// A `tap` or `each` block's value and an `if` branch whose `if` is a dropped statement are dropped too, so a call there checks clean; a method's tail is the method's value and keeps the refusal.
#[test]
fn a_dropped_block_or_branch_value_checks_clean() {
    let errors = constant_errors(
        "    Comment.all.to_a.tap { |rs| ActiveRecord::Associations::Preloader.new(records: rs, associations: :article).call }\n    if Comment.count > 0\n      ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n    end\n    nil\n",
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

/// An application `tap` may answer its block's value. `call` is still
/// nil, and the class exists, so check stays clean.
#[test]
fn an_app_defined_tap_checks_clean() {
    let errors = constant_errors_with(
        "    Basket.new.tap { ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call }\n    nil\n",
        &[("app/models/basket.rb", "class Basket\n  def tap\n    yield\n  end\nend\n")],
    );
    assert!(errors.is_empty(), "{errors:#?}");
}

/// `scope:` is not a batch load. The class exists, so check is clean,
/// and the call stays `Preloader.new`. MRI raises unknown-keyword
/// `ArgumentError` (message includes "scope").
#[test]
fn a_scope_option_is_not_rewritten() {
    let errors = constant_errors(
        "    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article, scope: Article.where(id: 1)).call\n    nil\n",
    );
    assert!(errors.is_empty(), "{errors:#?}");
    let sends = lowered_sends(
        "    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article, scope: Comment.where(id: 1)).call\n    nil\n",
    );
    assert!(!sends.iter().any(|m| m == "preload_associations"), "{sends:?}");
    assert!(sends.iter().any(|m| m == "new"), "{sends:?}");
}

fn lowered_sends(body: &str) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n  create_table \"comments\" do |t|\n    t.integer \"article_id\"\n  end\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/application_record.rb"), b"class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/comment.rb"), b"class Comment < ApplicationRecord\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/note.rb"), b"class Note\n  def article = nil\nend\n".to_vec());
    tree.insert(PathBuf::from("app/models/digest.rb"), format!("class Digest\n  def self.run\n{body}  end\nend\n").into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let digest = app.library_classes.iter().find(|c| c.name.0.as_str() == "Digest").expect("Digest");
    let mut out = Vec::new();
    fn walk(e: &roundhouse::Expr, out: &mut Vec<String>) {
        if let roundhouse::expr::ExprNode::Send { method, .. } = &*e.node {
            out.push(method.as_str().to_string());
        }
        e.node.for_each_child(&mut |c| walk(c, out));
    }
    for m in &digest.methods {
        walk(&m.body, &mut out);
    }
    out
}

/// Lowering follows typing's decision: a call typing refused (records of a
/// class with no table) is not rewritten into `preload_associations`, even
/// when the tree is emitted past its errors.
#[test]
fn a_refused_call_is_not_rewritten() {
    let sends = lowered_sends("    ActiveRecord::Associations::Preloader.new(records: [Note.new], associations: :article).call\n    nil\n");
    assert!(!sends.iter().any(|m| m == "preload_associations"), "{sends:?}");
    let admitted = lowered_sends("    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n    nil\n");
    assert!(admitted.iter().any(|m| m == "preload_associations"), "{admitted:?}");
}
