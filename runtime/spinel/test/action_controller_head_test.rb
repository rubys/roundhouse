require "minitest/autorun"
require_relative "test_helper"

class ActionControllerHeadTest < Minitest::Test
  class TestController < ActionController::Base
  end

  def setup
    @controller = TestController.new
  end

  def test_options_set_normalized_headers_location_and_content_type
    options = {
      "x-custom_header" => 17,
      location: "/articles/7",
      content_type: "text/plain; charset=utf-8",
    }

    assert_equal true, @controller.head(:created, options)
    assert_equal 201, @controller.status
    assert_equal "", @controller.body
    assert_equal "/articles/7", @controller.location, "location"
    assert_equal "text/plain", @controller.content_type, "content type"
    assert_equal "X-Custom-Header", @controller.headers.key_at(0)
    assert_equal "17", @controller.headers.val_at(0)
    assert_equal({ "x-custom_header" => 17 }, options, "remaining options")
  end

  def test_resolves_mime_symbol_content_type
    @controller.head(:ok, { content_type: :json })
    assert_equal "application/json", @controller.content_type
  end

  def test_invalid_mime_symbol_does_not_mutate_controller_or_options
    options = {
      location: "/articles/7",
      content_type: :unknown_head_mime,
      "x-custom" => "value",
    }

    assert_raises(ArgumentError) { @controller.head(:created, options) }

    assert_equal 200, @controller.status
    assert_nil @controller.location
    assert_equal 0, @controller.headers.size
    refute @controller.performed?
    assert_equal({
      location: "/articles/7",
      content_type: :unknown_head_mime,
      "x-custom" => "value",
    }, options)
  end

  def test_defaults_nil_status_to_ok
    assert_equal true, @controller.head(nil)
    assert_equal 200, @controller.status
    assert_equal "", @controller.body
    assert_equal "text/html", @controller.content_type
  end

  def test_preserves_a_preexisting_media_type
    @controller.content_type = "application/problem+json; charset=utf-8"
    @controller.head(:ok, { content_type: "text/plain" })
    assert_equal "application/problem+json", @controller.content_type
  end

  def test_defaults_when_the_preexisting_media_type_is_empty
    @controller.content_type = ""
    @controller.head(:ok)
    assert_equal "text/html", @controller.content_type
  end

  def test_uses_registered_mime_for_negotiated_format
    @controller.request_format = :xml
    @controller.head(:ok)
    assert_equal "application/xml", @controller.content_type
  end

  def test_accepts_integer_statuses
    assert_equal true, @controller.head(418)
    assert_equal 418, @controller.status
  end

  def test_omits_content_type_for_bodyless_status_classes
    [100, 199, 204, 205, 304].each do |status|
      controller = ActionController::Base.new
      controller.head(status)
      assert_equal "", controller.content_type, "status #{status}"
      assert_equal "", controller.body, "status #{status}"
    end
  end

  def test_rejects_hash_status
    error = assert_raises(ArgumentError) { @controller.head(location: "/wrong") }
    assert_includes error.message, "not a valid value for `status`"
    refute @controller.performed?
  end

  def test_raises_double_render_after_render_without_replacing_body
    @controller.render("partial output")
    assert_equal "partial output", @controller.body

    assert_raises(AbstractController::DoubleRenderError) { @controller.head(:no_content) }
    assert_equal 200, @controller.status
    assert_equal "partial output", @controller.body
  end

  def test_rejects_unknown_status_symbols
    assert_raises(ArgumentError) { @controller.head(:not_a_status) }
    refute @controller.performed?
  end
end
