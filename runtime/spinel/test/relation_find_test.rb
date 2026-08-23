# `Relation#find` raises, and `ActionResponse#parsed_body` decodes.
#
# Minitest-shaped: a CRuby-only framework test, quarantined out of the
# spin shape by `project.rs::spin_shape` (a `Minitest::Test` subclass is
# CRuby-only regardless), same as dom_test.rb beside it.
#
# WHY THIS FILE EXISTS. Both behaviors are RUNTIME, so the compiler's
# own test suite cannot see them, and both are the kind that fails
# quietly: a `find` that answers nil turns a missing record into a
# NoMethodError several frames from the lookup, and a `parsed_body` that
# is not there turns a JSON assertion into a harness error the test
# author reads as a transpiler gap.
require "minitest/autorun"
require_relative "test_helper"
require_relative "fixtures/articles"
require_relative "fixtures/comments"

class RelationFindTest < Minitest::Test
  def setup
    SchemaSetup.reset! if defined?(SchemaSetup)
  end

  def relation
    ActiveRecord::Relation.new(Article)
  end

  def test_find_answers_the_row_with_that_id
    found = relation.find(ArticlesFixtures.one.id)
    assert_equal ArticlesFixtures.one.id, found.id
  end

  # Rails' whole distinction between `find` and `find_by`. This answered
  # nil until 2026-08-22.
  def test_find_raises_when_there_is_no_such_row
    assert_raises(ActiveRecord::RecordNotFound) { relation.find(987_654_321) }
  end

  def test_find_by_still_answers_nil
    assert_nil relation.find_by(id: 987_654_321)
  end

  # A terminal must leave the relation as it found it — the id predicate
  # is popped, including on the raising path, so a rescued lookup does
  # not silently narrow every later use of that relation to one row.
  def test_a_raising_find_leaves_the_relation_unnarrowed
    rel = relation
    assert_raises(ActiveRecord::RecordNotFound) { rel.find(987_654_321) }
    assert_operator rel.count, :>, 0
  end
end

class ParsedBodyTest < Minitest::Test
  def response_for(body, content_type)
    ActionResponse.new(
      status: 200,
      body: body,
      location: nil,
      flash: ActionDispatch::Flash.new,
      cookies: ActionController::CookieJar.new,
      content_type: content_type
    )
  end

  def test_a_json_body_decodes
    r = response_for('[{"name":"David"}]', "application/json")
    assert_equal "David", r.parsed_body.first["name"]
  end

  # Refusing beats answering the raw String: `parsed_body["k"]` on a
  # String is a TypeError three lines from where the test went wrong.
  def test_a_non_json_body_says_so
    r = response_for("<h1>hi</h1>", "text/html; charset=utf-8")
    err = assert_raises(RuntimeError) { r.parsed_body }
    assert_includes err.message, "parsed_body"
  end
end
