use super::emit_and_run;

/// `before_validation :m, on: :create` / `after_validation :m, on: :update`
/// guard the callback by the record's state. The guard used to be a bare
/// `new_record?`, which the strict targets spelled as a call the model
/// can't make: rust `new_record_pred()` (E0425, cannot find function) and
/// go `self.NewRecord` (no such field), since `new_record?` lives on the
/// runtime Base that a rust or go struct doesn't inherit. It is now
/// `self.persisted?`, synthesized per model on every target.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "class Article < ApplicationRecord\n",
        "class Article < ApplicationRecord\n  before_validation :mark_created, on: :create\n  after_validation :mark_updated, on: :update\n\n  def mark_created\n    @seen = \"create\"\n  end\n\n  def mark_updated\n    @seen = \"update\"\n  end\n\n  def seen\n    @seen\n  end\n",
    )
}

#[test]
fn strict_targets_guard_on_the_models_own_persisted() {
    let (rust, _) = app().emit(roundhouse::project::BuildTarget::Rust);
    let model = std::fs::read_to_string(rust.join("src/models/article.rs")).unwrap();
    assert!(!model.contains("if new_record_pred()") && !model.contains("!(new_record_pred())"), "bare new_record_pred() in:\n{model}");
    assert!(model.contains("self.persisted_pred()"), "{model}");

    let (go, _) = app().emit(roundhouse::project::BuildTarget::Go);
    let model = std::fs::read_to_string(go.join("app/v2/article.go")).unwrap();
    assert!(!model.contains("self.NewRecord {") && !model.contains("!(self.NewRecord)"), "self.NewRecord in:\n{model}");
}

/// Rails 8.1.4: saving a new record runs only the `on: :create` callback,
/// and saving it again runs only the `on: :update` one.
#[test]
fn validation_callbacks_run_in_their_context() {
    app()
        .run_ruby(
            r#"a = Article.new(title: "Hello", body: "A body long enough to pass validation.")
a.save!
created = a.seen
a.title = "Hi"
a.save!
raise "seen: #{[created, a.seen].inspect}" unless [created, a.seen] == ["create", "update"]
puts "validation callback contexts passed"
"#,
        )
        .assert_passes();
}

/// Rails runs the validation callbacks inside `valid?`, so a direct
/// `valid?` runs them in the record's context as `save` does (#1140).
/// Answers are Rails 8.1.4's.
#[test]
fn valid_runs_the_validation_callbacks() {
    app()
        .run_ruby(
            r#"a = Article.new(title: "Hello", body: "A body long enough to pass validation.")
a.valid?
created = a.seen
a.save!
a.valid?
raise "seen: #{[created, a.seen].inspect}" unless [created, a.seen] == ["create", "update"]
puts "valid? validation callbacks passed"
"#,
        )
        .assert_passes();
}

/// The belongs_to autosave is a save step, not a validation callback, so
/// a direct `valid?` leaves an unsaved parent unsaved, as Rails 8.1.4
/// does. `save!` still saves it first and takes its id.
#[test]
fn valid_does_not_save_an_unsaved_belongs_to_target() {
    emit_and_run::real_blog()
        .run_ruby(
            r#"before = Article.count
article = Article.new(title: "Parent", body: "A body long enough to pass validation.")
comment = Comment.new(article: article, commenter: "Ann", body: "Hello there")
comment.valid?
checked = [Article.count - before, article.persisted?]
raise "after valid?: #{checked.inspect}" unless checked == [0, false]
comment.save!
saved = [Article.count - before, comment.article_id == article.id]
raise "after save!: #{saved.inspect}" unless saved == [1, true]
puts "belongs_to autosave waits for save"
"#,
        )
        .assert_passes();
}

/// A belongs_to `default:` applies only when no target is assigned, as
/// Rails' `association.default` checks the reader. An unsaved target
/// assigned through the writer has a zero foreign key until `save`
/// autosaves it, so a direct `valid?` must not replace it.
#[test]
fn valid_keeps_an_assigned_unsaved_target_over_the_default() {
    emit_and_run::real_blog()
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            "  belongs_to :article, default: -> { Article.order(:id).first }\n  before_validation { self.article = Article.order(:id).first if commenter == \"swap\" }\n",
        )
        .run_ruby(
            r#"fallback = Article.create!(title: "Fallback", body: "A body long enough to pass validation.")
unassigned = Comment.new(commenter: "Bo", body: "Hello there")
unassigned.valid?
raise "default not applied: #{unassigned.article_id.inspect}" unless unassigned.article_id == fallback.id
article = Article.new(title: "Parent", body: "A body long enough to pass validation.")
comment = Comment.new(article: article, commenter: "Ann", body: "Hello there")
comment.valid?
raise "replaced by the default: #{comment.article.title}" unless comment.article.equal?(article)
comment.save!
saved = [article.persisted?, comment.article_id == article.id]
raise "after save!: #{saved.inspect}" unless saved == [true, true]
swapped = Comment.new(article: article, commenter: "swap", body: "Hello there")
swapped.valid?
raise "the model's own assignment was guarded: #{swapped.article_id.inspect}" unless swapped.article_id == fallback.id
puts "assigned target kept over the default"
"#,
        )
        .assert_passes();
}
