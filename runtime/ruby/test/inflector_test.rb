require_relative "test_helper"

# Direct unit tests for `runtime/ruby/inflector.rb`. Promoted from
# fixtures/spinel-blog/test/runtime/inflector_test.rb (which is
# blog-coupled via test_helper); this version is framework-only.
class InflectorTest < Minitest::Test
  def test_pluralize_singular_count
    assert_equal "1 comment", Inflector.pluralize(1, "comment")
  end

  def test_pluralize_zero_count
    assert_equal "0 comments", Inflector.pluralize(0, "comment")
  end

  def test_pluralize_plural_count
    assert_equal "2 comments", Inflector.pluralize(2, "comment")
    assert_equal "42 articles", Inflector.pluralize(42, "article")
  end

  def test_pluralize_negative_count
    # Pluralize uses `count == 1` as the singular check, so any
    # value other than exactly 1 takes the plural form. Negative
    # counts surface in `assert_difference("Comment.count", -1)`-
    # style messages where the diff is plural.
    assert_equal "-1 comments", Inflector.pluralize(-1, "comment")
  end

  def test_pluralize_formatted_count_preserves_rails_singular_boundaries
    assert_equal "1 word", Inflector.pluralize_formatted("1", "word")
    assert_equal "1.00 word", Inflector.pluralize_formatted("1.00", "word")
    assert_equal "01 words", Inflector.pluralize_formatted("01", "word")
    assert_equal "1.01 words", Inflector.pluralize_formatted("1.01", "word")
    assert_equal "1,001 words", Inflector.pluralize_formatted("1,001", "word")
    assert_equal " words", Inflector.pluralize_formatted("", "word")
    assert_equal "2\n1.0\n word", Inflector.pluralize_formatted("2\n1.0\n", "word")
  end
end
