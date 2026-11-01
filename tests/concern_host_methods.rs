//! A controller concern's bare call may name a method the module never
//! defines: the includer does, or a sibling concern the includer also
//! mixes in. Rails' authentication generator has the shape — an inner
//! module's `current_user` calls `resume_session`, which the outer
//! module defines — and an unresolved call there is `untyped`, which
//! then reaches every controller that reads `current_user`.
//!
//! 1. **One includer.** The module's bodies are typed with the includer
//!    as `self`. The second typing pass (taken once the module has any
//!    ivar environment) fell back to the module as `self` and undid it.
//! 2. **Several includers that agree.** `self` stays the module, and the
//!    module is lent the methods every includer resolves to one type.
//! 3. **Several includers that disagree** (`Comment?` in one, `Comment`
//!    in the other). Nothing is lent; the call stays the gradual escape
//!    it was.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :articles do |t|\n    t.string :title\n  end\n  \
    create_table :comments do |t|\n    t.integer :article_id\n    t.string :body\n  end\nend\n";

const ARTICLE: &str = "class Article < ApplicationRecord\n  has_many :comments\nend\n";
const COMMENT: &str = "class Comment < ApplicationRecord\n  belongs_to :article\nend\n";

/// Calls `resume_comment`, which it does not define.
const INNER: &str = r#"module Impersonation
  extend ActiveSupport::Concern

  def current_article
    resume_comment&.article
  end
end
"#;

const OUTER: &str = r#"module Authentication
  extend ActiveSupport::Concern

  include Impersonation

  private

  def resume_comment
    Comment.find_by(id: cookies.signed[:comment_id])
  end
end
"#;

const NOTES: &str = r#"class NotesController < ApplicationController
  before_action :set_note, only: [:show]

  def show
  end

  private

  def set_note
    @note = current_article.comments.find(params[:id])
  end
end
"#;

const BASE: &[(&str, &str)] = &[
    ("db/schema.rb", SCHEMA),
    ("config/routes.rb", "Rails.application.routes.draw do\n  resources :notes, only: [:show]\nend\n"),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/article.rb", ARTICLE),
    ("app/models/comment.rb", COMMENT),
    ("app/controllers/concerns/impersonation.rb", INNER),
    ("app/controllers/notes_controller.rb", NOTES),
    ("app/views/notes/show.html.erb", "<p><%= @note.body %></p>\n"),
];

fn diagnostics(extra: &[(&str, &str)]) -> Vec<String> {
    let tree: HashMap<PathBuf, Vec<u8>> = BASE
        .iter()
        .chain(extra)
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app).into_iter().map(|d| d.to_string()).collect()
}

#[test]
fn one_includer_keeps_the_includer_as_self() {
    let found = diagnostics(&[
        ("app/controllers/concerns/authentication.rb", OUTER),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Authentication\nend\n",
        ),
    ]);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn several_includers_that_agree_lend_the_module_their_method() {
    let found = diagnostics(&[
        ("app/controllers/concerns/authentication.rb", OUTER),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Authentication\nend\n",
        ),
        (
            "app/controllers/api/base_controller.rb",
            "class Api::BaseController < ActionController::Base\n  include Authentication\nend\n",
        ),
    ]);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn several_includers_that_disagree_lend_nothing() {
    let found = diagnostics(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  include Impersonation\n\n  \
             private\n\n  def resume_comment\n    Comment.find_by(id: 1)\n  end\nend\n",
        ),
        (
            "app/controllers/api/base_controller.rb",
            "class Api::BaseController < ActionController::Base\n  include Impersonation\n\n  \
             private\n\n  def resume_comment\n    Comment.find(1)\n  end\nend\n",
        ),
    ]);
    assert!(
        found.iter().any(|d| d.contains("gradual_untyped")),
        "the call has no single answer and must stay gradual: {found:#?}"
    );
    assert!(!found.iter().any(|d| d.starts_with("error")), "{found:#?}");
}
