require_relative "../test_helper"
require "inflector"

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
end
