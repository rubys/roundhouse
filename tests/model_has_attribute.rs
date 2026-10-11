//! `has_attribute?` on a model instance.
//!
//! A model guarding reads of newer columns with
//! `other.has_attribute?(:payment_attempt_id)`. The runtime
//! base implements it (schema columns; a column a `select` left out
//! reads false), but the analyzer's model surface did not list it, so
//! every call was `no known method`. `association(:name)` has no runtime
//! implementation and stays refused.

use std::collections::HashMap;
use std::path::PathBuf;

#[test]
fn has_attribute_resolves_on_a_model() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/structure.sql", "CREATE TABLE widgets (\n    id bigint NOT NULL,\n    name character varying\n);\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        (
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  #: (Widget) -> bool\n  def named?(other)\n    other.has_attribute?(:name) && has_attribute?(:name)\n  end\n\n  #: (Widget) -> untyped\n  def proxy(other) = other.association(:parts)\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = roundhouse::analyze::diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(!errors.iter().any(|e| e.contains("has_attribute?")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("`association`")), "{errors:?}");
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The emitted program answers `has_attribute?` from the schema.
#[test]
fn has_attribute_runs_on_a_model() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  #: (Article) -> bool\n  def same_title_column?(other)\n    other.has_attribute?(:title) && has_attribute?(:title)\n  end\n\n  #: -> bool\n  def knows_missing? = has_attribute?(:no_such_column)\n\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Probe", body: "A body long enough to pass.")
raise "known column not found" unless article.same_title_column?(article)
raise "missing column found" if article.knows_missing?
"#,
        )
        .assert_passes();
}
