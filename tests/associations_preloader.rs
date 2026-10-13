//! `ActiveRecord::Associations::Preloader.new(records:, associations:).call`
//! checks clean when its value is discarded and its records are one
//! model's (it lowers to that model's `preload_associations`), and keeps
//! its refusal otherwise, since no target ships the class.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn constant_errors(body: &str) -> Vec<String> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
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

/// Rails' `call` answers the loaders; `preload_associations` answers nil,
/// so a call whose value is used (a condition, a method's tail) keeps
/// the refusal.
#[test]
fn a_used_value_keeps_the_refusal() {
    let errors = constant_errors(
        "    if ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n      1\n    end\n    ActiveRecord::Associations::Preloader.new(records: Comment.all.to_a, associations: :article).call\n",
    );
    assert_eq!(errors.len(), 2, "{errors:#?}");
}

#[test]
fn mixed_records_keep_the_refusal() {
    let errors = constant_errors(
        "    ActiveRecord::Associations::Preloader.new(records: [Comment.first, Article.first], associations: :article).call\n    nil\n",
    );
    assert_eq!(errors.len(), 1, "{errors:#?}");
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
