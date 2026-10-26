use super::emit_and_run;

/// `uniqueness:` on a model with a `default_scope`. Rails'
/// UniquenessValidator queries `finder_class.unscoped`, so a value held
/// only by a row the default scope hides still collides, and the record
/// itself is still excluded by its key. It was a reported gap. The
/// lowered check now queries `Model.unscoped.where(…)`. Answers are Rails
/// 8.1.4's for the same model and rows.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"slug\"\n    t.boolean \"deleted\", default: false, null: false\n  end\nend\n",
        )
        .write(
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  default_scope { where(deleted: false) }\n  validates :slug, uniqueness: true\nend\n",
        )
}

#[test]
fn uniqueness_on_a_default_scoped_model_queries_unscoped_like_rails() {
    app()
        .run_ruby(
            r#"live = Post.new
live.slug = "live"
live.save!
hidden = Post.new
hidden.slug = "gone"
hidden.deleted = true
hidden.save!
got = %w[live gone fresh].map do |slug|
  p = Post.new
  p.slug = slug
  p.valid?
  p.errors.to_a
end
hidden.valid?
got << hidden.errors.to_a
want = [["Slug has already been taken"], ["Slug has already been taken"], [], []]
raise "errors: #{got.inspect}" unless got == want
puts "unscoped uniqueness passed"
"#,
        )
        .assert_passes();
}

/// `Model.unscoped { … }` runs the block with the default scope removed
/// and returns the block's value, as Rails does. Default scopes are not
/// lowered, so the block reads every row already. Answers are Rails
/// 8.1.4's for the same rows (a visible "live" and a hidden "gone").
#[test]
fn unscoped_with_a_block_returns_the_block_value_like_rails() {
    app()
        .run_ruby(
            r#"live = Post.new
live.slug = "live"
live.save!
hidden = Post.new
hidden.slug = "gone"
hidden.deleted = true
hidden.save!
got = [
  Post.unscoped { Post.where(deleted: true).count },
  Post.unscoped { Post.where(slug: "gone").count },
  Post.unscoped { "value" },
  Post.unscoped.where(deleted: true).count,
]
raise "got #{got.inspect}" unless got == [1, 1, "value", 1]
puts "unscoped block passed"
"#,
        )
        .assert_passes();
}
