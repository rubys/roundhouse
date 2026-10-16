# frozen_string_literal: true
require_relative "../test_helper"

# `ActionDispatch::MimeNegotiation.should_apply_vary_header?` and the
# `Vary: Accept` that `render` / `send_data` then write. Each expected
# answer is whether a Rails 8.1.4 app's response to the same request
# carried `vary: Accept`, through the full default middleware stack.
class MimeNegotiationTest < Minitest::Test
  BROWSER = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"

  def vary?(accept, xhr: "", content_type: "", format_param: false)
    ActionDispatch::MimeNegotiation.should_apply_vary_header?(accept, xhr, content_type, format_param)
  end

  def test_a_non_browser_accept_varies
    assert vary?("*/*")
    assert vary?("text/html")
    assert vary?("application/json")
  end

  def test_a_browser_accept_or_none_does_not
    refute vary?(BROWSER)
    refute vary?("*/* ,text/html")
    refute vary?("text/html;q=0.9,*/*")
    refute vary?("")
    refute vary?("  ")
  end

  # `/page.html`, `?format=html`, `?format=` and a route's own format
  # all read the format param instead of Accept.
  def test_a_format_param_does_not
    refute vary?("text/html", format_param: true)
  end

  def test_an_xhr_varies_with_an_accept_or_a_body_type
    assert vary?("text/html, */*", xhr: "XMLHttpRequest")
    assert vary?("", xhr: "XMLHttpRequest", content_type: "text/plain")
    refute vary?("", xhr: "XMLHttpRequest")
  end

  class TestController < ActionController::Base
    def process_action(action_name)
    end
  end

  def controller(vary)
    c = TestController.new
    c.vary_on_accept = vary
    c
  end

  def test_render_and_send_data_write_vary_accept
    c = controller(true)
    c.render("<p>hi</p>")
    assert_equal "Accept", c.headers["Vary"]
    c = controller(true)
    c.send_data("bytes", type: "text/plain")
    assert_equal "Accept", c.headers["Vary"]
    c = controller(false)
    c.render("<p>hi</p>")
    assert_nil c.headers["Vary"]
  end

  def test_a_redirect_or_head_writes_none
    c = controller(true)
    c.redirect_to("/page")
    assert_nil c.headers["Vary"]
    c = controller(true)
    c.head(:ok)
    assert_nil c.headers["Vary"]
  end

  def test_an_action_vary_is_kept
    c = controller(true)
    c.headers["Vary"] = "X-Inertia"
    c.render("<p>hi</p>")
    assert_equal "X-Inertia", c.headers["Vary"]
  end
end
