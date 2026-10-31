//! Relation methods the PostgreSQL fork specs reach, on SQLite: `and`
//! (with HAVING, and refusing structurally different relations),
//! `unscope` of the order, limit and offset, an offset with no limit, and
//! `readonly`, whose records refuse to be saved, destroyed, updated by
//! column or touched.

use super::emit_and_run;

#[test]
fn and_unscope_parts_offset_and_readonly_answer_as_rails_does() {
    emit_and_run::real_blog()
        .write(
            "test/models/relation_parts_test.rb",
            r#"require "test_helper"

class RelationPartsTest < ActiveSupport::TestCase
  test "and, unscope of parts, offset alone and readonly" do
    Comment.delete_all
    Article.delete_all
    %w[a b c d].each { |t| Article.create!(title: t, body: "Long enough body text") }
    left = Article.where(title: %w[a b c])
    right = Article.where(title: %w[b c d])
    assert_equal %w[b c], left.and(right).order(:title).pluck(:title)
    assert_equal %w[a b c], left.order(:title).pluck(:title)
    paged = Article.order(title: :desc).limit(1).offset(1)
    assert_equal %w[c], paged.pluck(:title)
    assert_equal %w[a b c d], paged.unscope(:order, :limit, :offset).pluck(:title).sort
    assert_equal %w[c d], Article.order(:title).offset(2).pluck(:title)
    held = Article.readonly.find_by(title: "a")
    assert held.readonly?
    assert_not Article.find_by(title: "a").readonly?
    assert_raises(ActiveRecord::ReadOnlyRecord) { held.update!(title: "z") }
    assert_raises(ActiveRecord::ReadOnlyRecord) { held.destroy }
    assert_raises(ActiveRecord::ReadOnlyRecord) { held.update_column(:title, "z") }
    assert_raises(ActiveRecord::ReadOnlyRecord) { held.update_columns(title: "z") }
    assert_raises(ActiveRecord::ReadOnlyRecord) { held.touch }
    assert_equal 1, Article.where(title: "a").count
    assert_equal [], left.and(Article.none).unscope(:where).pluck(:title)
    grouped = left.group(:title).and(Article.group(:title).having("title <> 'a'"))
    assert_equal %w[b c], grouped.order(:title).pluck(:title)
    assert_raises(ArgumentError) { Article.limit(1).and(left) }
    assert_raises(ArgumentError) { left.and(Article.order(:title)) }
    assert_equal %w[b c], left.order(:title).and(right).pluck(:title)
  end
end
"#,
        )
        .run_test("test/models/relation_parts_test.rb")
        .assert_passes();
}
