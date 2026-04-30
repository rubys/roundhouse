require_relative "../test_helper"

class ParametersTest < Minitest::Test
  def test_reads_value_by_symbol_or_string
    p = ActionController::Parameters.new("title" => "Hi")
    assert_equal "Hi", p[:title]
    assert_equal "Hi", p["title"]
  end

  def test_key_predicate
    p = ActionController::Parameters.new(title: "x")
    assert p.key?(:title)
    assert p.key?("title")
    refute p.key?(:body)
  end

  def test_to_h_returns_plain_hash
    p = ActionController::Parameters.new(title: "x", nested: { a: 1 })
    assert_equal({ title: "x", nested: { a: 1 } }, p.to_h)
  end

  def test_require_returns_nested_parameters
    p = ActionController::Parameters.new(article: { title: "x", body: "y" })
    nested = p.require(:article)
    assert_kind_of ActionController::Parameters, nested
    assert_equal "x", nested[:title]
  end

  def test_require_raises_on_missing
    p = ActionController::Parameters.new
    assert_raises(ActionController::ParameterMissing) { p.require(:article) }
  end

  def test_require_raises_on_empty_hash
    p = ActionController::Parameters.new(article: {})
    assert_raises(ActionController::ParameterMissing) { p.require(:article) }
  end

  def test_permit_filters_to_listed_keys
    p = ActionController::Parameters.new(title: "x", body: "y", evil: "z")
    permitted = p.permit([:title, :body])
    assert_equal({ title: "x", body: "y" }, permitted.to_h)
  end

  def test_permit_omits_missing_keys
    p = ActionController::Parameters.new(title: "x")
    permitted = p.permit([:title, :body])
    assert_equal({ title: "x" }, permitted.to_h)
  end

  def test_string_keys_normalized_to_symbols
    p = ActionController::Parameters.new("article" => { "title" => "x" })
    nested = p.require(:article)
    assert_equal "x", nested[:title]
  end
end
