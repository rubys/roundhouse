//! Literal model reflection is grounded only when both the dispatcher
//! and target are proven to be Rails' public generated surface.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn emitted(files: &[roundhouse::emit::EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("missing {suffix}"))
        .content
        .clone()
}

fn output() -> Vec<roundhouse::emit::EmittedFile> {
    let sources = [
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "articles" do |t|; t.string "title"; end
  create_table "comments" do |t|; t.integer "article_id"; t.string "body"; end
  create_table "dispatcher_posts" do |t|; t.string "title"; end
  create_table "reader_posts" do |t|; t.string "title"; end
  create_table "private_posts" do |t|; t.string "title"; end
  create_table "protected_posts" do |t|; t.string "title"; end
  create_table "included_posts" do |t|; t.string "title"; end
  create_table "inherited_posts" do |t|; t.string "title"; end
end
"#,
        ),
        (
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  scope :recent, -> { order(:id) }
end
"#,
        ),
        (
            "app/models/article.rb",
            r#"class Article < ApplicationRecord
  has_many :comments
end
"#,
        ),
        (
            "app/controllers/articles_controller.rb",
            r#"class ArticlesController < ApplicationController
  def show
    @article = Article.find(params[:id])
    @comments = @article.public_send(:comments).public_send(:recent)
  end
end
"#,
        ),
        (
            "app/models/dispatcher_post.rb",
            r#"class DispatcherPost < ApplicationRecord
  scope :recent, -> { order(:id) }
  def self.public_send(name); where(title: name.to_s); end
  def self.probe; self.public_send(:recent); end
end
"#,
        ),
        (
            "app/models/reader_post.rb",
            r#"class ReaderPost < ApplicationRecord
  has_many :comments, foreign_key: :article_id
  def comments; []; end
  def probe; comments.public_send(:recent); end
end
"#,
        ),
        (
            "app/models/private_post.rb",
            r#"class PrivatePost < ApplicationRecord
  scope :recent, -> { order(:id) }
  def self.recent; where(title: "private"); end
  private_class_method :recent
  def self.probe; self.public_send(:recent); end
end
"#,
        ),
        (
            "app/models/concerns/dispatch_overrides.rb",
            r#"module DispatchOverrides
  extend ActiveSupport::Concern
  class_methods do
    def recent; where(title: "included"); end
  end
end
"#,
        ),
        (
            "app/models/protected_post.rb",
            r#"class ProtectedPost < ApplicationRecord
  scope :recent, -> { order(:id) }
  def self.recent; where(title: "protected"); end
  class << self; protected :recent; end
  def self.probe; self.public_send(:recent); end
end
"#,
        ),
        (
            "app/models/included_post.rb",
            r#"class IncludedPost < ApplicationRecord
  include DispatchOverrides
  scope :recent, -> { order(:id) }
  def self.probe; self.public_send(:recent); end
end
"#,
        ),
        (
            "app/models/inherited_post.rb",
            r#"class InheritedPost < ApplicationRecord
  scope :recent, -> { order(:id) }
  def self.probe; self.public_send(:recent); end
end
"#,
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = sources
        .into_iter()
        .map(|(path, body)| (PathBuf::from(path), body.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    // Keep this fixture independently ingestible, then model the recorded
    // app parent edge whose override the grounding decision must inspect.
    app.models
        .iter_mut()
        .find(|m| m.name.0.as_str() == "InheritedPost")
        .expect("InheritedPost")
        .parent = Some(roundhouse::ident::ClassId(roundhouse::ident::Symbol::from(
        "DispatcherPost",
    )));
    let mut out = ruby::emit_lowered_models(&app);
    out.extend(ruby::emit_lowered_controllers(&app));
    out
}

#[test]
fn generated_public_association_and_scope_form_a_threaded_direct_chain() {
    let src = emitted(&output(), "app/controllers/articles_controller.rb");
    assert!(
        !src.contains("public_send(:comments)"),
        "association stayed reflective:\n{src}"
    );
    assert!(
        !src.contains("public_send(:recent)"),
        "scope stayed reflective:\n{src}"
    );
    assert!(
        src.contains(
            "Comment.recent(ActiveRecord::Relation.new(Comment).where(article_id: @article.id)"
        ),
        "relation was not threaded:\n{src}"
    );
}

#[test]
fn custom_dispatcher_and_custom_association_reader_preserve_reflection() {
    let files = output();
    let dispatcher = emitted(&files, "app/models/dispatcher_post.rb");
    assert!(
        dispatcher.contains("public_send(:recent)"),
        "custom dispatcher was bypassed:\n{dispatcher}"
    );
    let reader = emitted(&files, "app/models/reader_post.rb");
    assert!(
        reader.contains("comments.public_send(:recent)"),
        "custom reader was bypassed:\n{reader}"
    );
}

#[test]
fn private_included_and_inherited_targets_preserve_reflection() {
    let files = output();
    for path in [
        "private_post.rb",
        "protected_post.rb",
        "included_post.rb",
        "inherited_post.rb",
    ] {
        let src = emitted(&files, path);
        assert!(
            src.contains("public_send(:recent)"),
            "override in {path} was bypassed:\n{src}"
        );
    }
}

#[test]
fn compiled_wrappers_do_not_bypass_custom_dispatch_or_protected_visibility() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy",
            r#"  has_many :comments, dependent: :destroy
  scope :recent_order, -> { order(:id) }
  def self.public_send(name)
    :class_dispatch_kept
  end
  def self.dispatch_probe
    self.public_send(:recent_order)
  end"#,
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article",
            r#"  belongs_to :article
  def public_send(name)
    :instance_dispatch_kept
  end
  def dispatch_probe
    self.public_send(:article)
  end
  def guarded_value
    :guarded
  end
  protected :guarded_value
  def guarded_send
    self.send(:guarded_value)
  end"#,
        )
        .run_ruby(
            r#"
raise "class dispatcher bypassed" unless Article.dispatch_probe == :class_dispatch_kept
comment = Comment.new
raise "instance dispatcher bypassed" unless comment.dispatch_probe == :instance_dispatch_kept
raise "protected send" unless comment.guarded_send == :guarded
raise "protected default visibility" if comment.respond_to?(:guarded_value)
raise "protected include_private" unless comment.respond_to?(:guarded_value, true)
puts "literal dispatch runtime parity passed"
"#,
        );
    run.assert_passes();
    assert!(
        run.stdout
            .contains("literal dispatch runtime parity passed")
    );
}
