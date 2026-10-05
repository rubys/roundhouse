//! A receiver-less `tag` is the view TagBuilder only when the class the
//! body belongs to has no `tag` of its own (`lower::tag_builder`). Kept
//! out of tests/emit_and_run.rs so concurrent appends there do not
//! conflict. Same harness.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// `labels` (the association target) and `stickers` (which points at it
/// through `tag_id`), ahead of the blog's own tables.
fn with_labels() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|",
            "  create_table \"labels\", force: :cascade do |t|\n    t.string \"title\"\n    t.integer \"owner_id\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"stickers\", force: :cascade do |t|\n    t.integer \"tag_id\", null: false\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"articles\", force: :cascade do |t|",
        )
        .write("app/models/label.rb", "class Label < ApplicationRecord\nend\n")
}

/// A model's own association. The time tracker this was found in checks
/// `tag.user_id == time_entry.user_id` on its join model; the pass
/// rewrote `tag.user_id` to `"<user-id></user-id>"`, so the check failed
/// for every row and every tagging was rejected, with nothing reported.
#[test]
fn a_bare_tag_in_a_model_is_its_association_not_the_tag_builder() {
    with_labels()
        .write(
            "app/models/sticker.rb",
            "class Sticker < ApplicationRecord\n  belongs_to :tag, class_name: \"Label\"\n\n  def tag_owner\n    tag.owner_id\n  end\nend\n",
        )
        .run_ruby(r#"
Sticker.delete_all
Label.delete_all
l = Label.create!(title: "urgent", owner_id: 7)
s = Sticker.create!(tag_id: l.id)
raise "tag reader became the tag builder: #{s.tag_owner.inspect}" unless s.tag_owner == 7
puts "model tag reader ok"
"#)
        .assert_passes();
}

/// A concern's body runs as its includer, so its bare `tag` is the
/// includer's association.
#[test]
fn a_bare_tag_in_a_concern_is_its_includers_association() {
    with_labels()
        .write(
            "app/models/concerns/taggable.rb",
            "module Taggable\n  def tag_title\n    tag.title\n  end\nend\n",
        )
        .write(
            "app/models/sticker.rb",
            "class Sticker < ApplicationRecord\n  include Taggable\n  belongs_to :tag, class_name: \"Label\"\nend\n",
        )
        .run_ruby(r#"
Sticker.delete_all
Label.delete_all
l = Label.create!(title: "urgent", owner_id: 7)
s = Sticker.create!(tag_id: l.id)
raise "concern tag became the tag builder: #{s.tag_title.inspect}" unless s.tag_title == "urgent"
puts "concern tag reader ok"
"#)
        .assert_passes();
}

/// A plain Ruby class's `attr_reader :tag`.
#[test]
fn a_bare_tag_in_a_plain_class_is_its_attr_reader() {
    with_labels()
        .write(
            "app/models/tag_holder.rb",
            "class TagHolder\n  attr_reader :tag\n\n  def initialize(tag)\n    @tag = tag\n  end\n\n  def tag_title\n    tag.title\n  end\nend\n",
        )
        .run_ruby(r#"
h = TagHolder.new(Label.new(title: "urgent", owner_id: 7))
raise "attr_reader tag became the tag builder: #{h.tag_title.inspect}" unless h.tag_title == "urgent"
puts "plain class tag reader ok"
"#)
        .assert_passes();
}

/// A model that really does use the builder has no `tag` of its own, so
/// the rewrite still applies there. It must not come out as a verbatim
/// `tag.span(…)` with nothing behind `tag` and only a warning to show for
/// it: either the builder runs, or `check` says it is unsupported.
#[test]
fn a_model_that_includes_tag_helper_is_never_silently_broken() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include ActionView::Helpers::TagHelper\n\n  def badge\n    tag.span(title)\n  end\n\n",
        )
        .run_ruby(r#"
out = Article.new(title: "hi").badge.to_s
raise "expected a span: #{out.inspect}" unless out == "<span>hi</span>"
puts "model tag builder ok"
"#);
    assert!(
        !run.errors.is_empty() || run.success,
        "silently broken: no error from check, and the emitted program failed\n\
         === stdout ===\n{}\n=== stderr ===\n{}",
        run.stdout,
        run.stderr,
    );
}
