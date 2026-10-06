require "minitest/autorun"
require_relative "../test_helper"

class AuthenticityTokenTest < Minitest::Test
  def setup
    ActionController::Current.controller = ActionController::Base.new
  end

  def teardown
    ActionController::Current.controller = nil
    ActionController::Current.request = nil
  end

  def test_masked_token_differs_from_the_session_secret_and_verifies
    first = ActionController::AuthenticityToken.masked
    second = ActionController::AuthenticityToken.masked
    secret = ActionController::Current.session[:_csrf_token].to_s
    refute_equal "", first
    refute_equal "", second
    refute_equal first, second
    refute_equal first, secret
    assert ActionController::AuthenticityToken.valid?(first, secret)
    assert ActionController::AuthenticityToken.valid?(second, secret)
    assert ActionController::AuthenticityToken.valid?(secret, secret)
  end

  def test_a_foreign_or_empty_token_is_refused
    ActionController::AuthenticityToken.masked
    secret = ActionController::Current.session[:_csrf_token].to_s
    refute ActionController::AuthenticityToken.valid?("", secret)
    refute ActionController::AuthenticityToken.valid?(secret.reverse, secret)
    refute ActionController::AuthenticityToken.valid?("!!!!", secret)
    refute ActionController::AuthenticityToken.valid?(secret, "")
  end

  def test_view_helpers_read_the_masked_token
    token = ActionView::ViewHelpers.form_authenticity_token
    secret = ActionController::Current.session[:_csrf_token].to_s
    assert ActionController::AuthenticityToken.valid?(token, secret)
    tags = ActionView::ViewHelpers.csrf_meta_tags
    assert_includes tags, %(name="csrf-token")
    hidden = ActionView::ViewHelpers.csrf_token_hidden_input
    assert_includes hidden, %(name="authenticity_token")
    hidden_token = hidden[/value="([^"]+)"/, 1]
    refute_equal secret, hidden_token
    assert ActionController::AuthenticityToken.valid?(hidden_token, secret)
  end
end
