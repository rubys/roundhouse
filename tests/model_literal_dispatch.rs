//! Literal model reflection is grounded only when both the dispatcher
//! and target are proven to be Rails' public generated surface.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::expr::ExprNode;
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

fn app() -> roundhouse::App {
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
  create_table "reopened_posts" do |t|; t.string "title"; end
  create_table "library_patch_posts" do |t|; t.string "title"; end
end
"#,
        ),
        (
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  belongs_to :article
  scope :recent, -> { order(:id) }
  def dispatch_probe; self.public_send(:article); end
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
        (
            "app/lib/reopened_dispatch.rb",
            r#"module ReopenedDispatch
  def marker; :first; end
end
module ReopenedDispatch
  def public_send(name); :dispatch_kept; end
end
"#,
        ),
        (
            "app/models/reopened_post.rb",
            r#"class ReopenedPost < ApplicationRecord
  include ReopenedDispatch
  has_many :comments, foreign_key: :article_id
  def probe; self.public_send(:comments); end
end
"#,
        ),
        (
            "app/models/library_patch_post.rb",
            r#"class LibraryPatchPost < ApplicationRecord
  has_many :comments, foreign_key: :article_id
  def probe; self.public_send(:comments); end
end
"#,
        ),
        (
            "app/lib/library_patch_post.rb",
            "class LibraryPatchPost\n  def public_send(name); :patch_kept; end\nend\n",
        ),
        (
            "app/lib/dispatch_guard.rb",
            "module DispatchGuard\n  def public_send(name); :initializer_kept; end\nend\n",
        ),
        (
            "config/initializers/dispatch_guard.rb",
            "Comment.prepend DispatchGuard\n",
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
    app
}

fn output() -> Vec<roundhouse::emit::EmittedFile> {
    let app = app();
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
fn generated_visibility_changes_are_refused_before_dispatch_grounding() {
    for (declaration, visibility) in [
        ("has_many :comments", "private :comments"),
        ("scope :recent, -> { order(:id) }", "private_class_method :recent"),
    ] {
        let files = HashMap::from([
            (PathBuf::from("db/schema.rb"), b"ActiveRecord::Schema.define do\n  create_table :articles do |t|; t.string :title; end\nend\n".to_vec()),
            (PathBuf::from("app/models/article.rb"), format!(
                "class Article < ApplicationRecord\n  {declaration}\n  {visibility}\nend\n"
            ).into_bytes()),
        ]);
        let error = ingest_app_from_tree(files).expect_err("generated visibility is not modeled");
        assert!(matches!(error, roundhouse::ingest::IngestError::Unsupported { message, .. }
            if message.contains("requires an already defined local method")), "{visibility}");
    }
    let app = app();
    let assocs = roundhouse::lower::scope_chain::build_assoc_registry(&app.models);
    for dispatcher in ["send", "__send__", "public_send"] {
        let source = format!("self.{dispatcher}(:comments)");
        let parsed = ruby_prism::parse(source.as_bytes());
        let statement = parsed.node().as_program_node().unwrap().statements().body().iter().next().unwrap();
        let mut body = roundhouse::ingest::ingest_expr(&statement, "probe.rb").unwrap();
        let ExprNode::Send { recv: Some(recv), .. } = &mut *body.node else { panic!("dispatch call") };
        recv.ty = Some(roundhouse::ty::Ty::Class {
            id: roundhouse::ident::ClassId(roundhouse::Symbol::from("Article")), args: vec![],
        });
        roundhouse::lower::scope_chain::ground_literal_model_dispatch(&mut body, &app, &assocs);
        assert_eq!(ruby::emit_expr(&body), "self.comments", "{dispatcher}");
    }
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
fn reopened_and_initializer_dispatchers_preserve_reflection() {
    let app = app();
    let assocs = roundhouse::lower::scope_chain::build_assoc_registry(&app.models);
    // Exercise the proof with known receiver types too: inference may keep
    // an overridden dispatch opaque before the grounding pass is reached.
    for (owner, target) in [
        ("Article", "comments"),
        ("ReopenedPost", "comments"),
        ("LibraryPatchPost", "comments"),
        ("Comment", "article"),
    ] {
        let source = format!("self.public_send(:{target})");
        let parsed = ruby_prism::parse(source.as_bytes());
        let statement = parsed
            .node()
            .as_program_node()
            .unwrap()
            .statements()
            .body()
            .iter()
            .next()
            .unwrap();
        let mut body = roundhouse::ingest::ingest_expr(&statement, "probe.rb").unwrap();
        let ExprNode::Send {
            recv: Some(recv), ..
        } = &mut *body.node
        else {
            panic!("expected dispatch call")
        };
        recv.ty = Some(roundhouse::ty::Ty::Class {
            id: roundhouse::ident::ClassId(roundhouse::Symbol::from(owner)),
            args: vec![],
        });
        roundhouse::lower::scope_chain::ground_literal_model_dispatch(&mut body, &app, &assocs);
        let source = ruby::emit_expr(&body);
        let expected = if owner == "Article" {
            format!("self.{target}")
        } else {
            format!("public_send(:{target})")
        };
        assert_eq!(source, expected, "{owner}");
    }
    let files = output();
    for (path, call) in [
        ("reopened_post.rb", "public_send(:comments)"),
        ("library_patch_post.rb", "public_send(:comments)"),
        ("comment.rb", "public_send(:article)"),
    ] {
        let src = emitted(&files, path);
        assert!(
            src.contains(call),
            "dispatcher in {path} was bypassed:\n{src}"
        );
    }
}

#[test]
fn initializer_installed_dispatch_runs_in_the_emitted_app() {
    let run = emit_and_run::real_blog()
        .write(
            "app/lib/dispatch_guard.rb",
            "module DispatchGuard\n  def public_send(name); :initializer_kept; end\nend\n",
        )
        .write(
            "config/initializers/dispatch_guard.rb",
            "Comment.prepend DispatchGuard\n",
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article",
            "  belongs_to :article\n  def dispatch_probe; self.public_send(:article); end",
        )
        .run_ruby(
            "raise 'initializer dispatcher bypassed' unless Comment.new.dispatch_probe == :initializer_kept\nputs 'initializer dispatch preserved'",
        );
    run.assert_passes();
    assert!(run.stdout.contains("initializer dispatch preserved"));
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
