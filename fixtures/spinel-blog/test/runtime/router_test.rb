require_relative "../test_helper"
require "action_dispatch"

class RouterTest < Minitest::Test
  TABLE = [
    { method: "GET",    pattern: "/articles",     controller: :articles_controller, action: :index   },
    { method: "GET",    pattern: "/articles/:id", controller: :articles_controller, action: :show    },
    { method: "POST",   pattern: "/articles",     controller: :articles_controller, action: :create  },
    { method: "DELETE", pattern: "/articles/:id", controller: :articles_controller, action: :destroy },
    { method: "POST",   pattern: "/articles/:article_id/comments", controller: :comments_controller, action: :create },
  ].freeze

  def test_matches_collection_get
    m = Router.match("GET", "/articles", TABLE)
    refute_nil m
    assert_equal :index, m[:action]
    assert_equal({}, m[:path_params])
  end

  def test_matches_member_get_and_captures_id
    m = Router.match("GET", "/articles/42", TABLE)
    refute_nil m
    assert_equal :show, m[:action]
    assert_equal "42", m[:path_params][:id]
  end

  def test_method_must_match
    assert_nil Router.match("PUT", "/articles", TABLE)
  end

  def test_returns_nil_when_path_does_not_match
    assert_nil Router.match("GET", "/articles/42/edit", TABLE)
    assert_nil Router.match("GET", "/foo", TABLE)
  end

  def test_captures_nested_resource_params
    m = Router.match("POST", "/articles/7/comments", TABLE)
    refute_nil m
    assert_equal :create, m[:action]
    assert_equal "7", m[:path_params][:article_id]
  end

  def test_method_is_case_insensitive
    m = Router.match("get", "/articles", TABLE)
    refute_nil m
    assert_equal :index, m[:action]
  end
end
