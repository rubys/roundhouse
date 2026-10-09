# Minitest-shaped, like hash_to_query_test.rb: a CRuby-only framework
# test of the ruby family's forgery check
# (runtime/request_forgery_protection.rb over the shared
# `ActionController::Base#verify_authenticity_token`). actionpack's
# rule, case by case: safe verbs pass, a token must match the session's
# in the param OR the X-CSRF-Token header (masked or unmasked), and a
# foreign or `null` Origin fails even with a good token.
require "minitest/autorun"
require_relative "test_helper"
require_relative "../runtime/request_forgery_protection"

class RequestForgeryProtectionTest < Minitest::Test
  # The harness turns the check off for app suites, as Rails' test.rb
  # does; this file is the one that tests it.
  def setup
    ActionController::Base.allow_forgery_protection = true
  end

  def teardown
    ActionController::Base.allow_forgery_protection = false
    ActionController::Current.controller = nil
    ActionController::Current.request = nil
  end

  def minted
    ActionController::AuthenticityToken.mint
  end

  def controller(method:, params: {}, headers: {}, session_token: :mint)
    env = { "REQUEST_METHOD" => method, "HTTP_HOST" => "chat.example.com" }
    headers.each { |k, v| env[k] = v }
    c = ActionController::Base.new
    token = session_token == :mint ? minted : session_token
    c.session[:_csrf_token] = token unless token.nil?
    ActionController::Current.controller = c
    c.params = params
    ActionController::Current.request = ActionDispatch::Request.new(env, params)
    c
  end

  def test_safe_verbs_pass_without_a_token
    assert controller(method: "GET", session_token: nil).verified_request?
    assert controller(method: "HEAD", session_token: nil).verified_request?
  end

  def test_a_post_without_a_token_is_refused_with_422
    c = controller(method: "POST")
    refute c.verified_request?
    c.verify_authenticity_token
    assert_equal 422, c.status
    assert c.performed?
  end

  def test_the_param_or_the_header_carries_the_token
    secret = minted
    c = controller(method: "GET", session_token: secret)
    masked = ActionController::AuthenticityToken.masked
    assert controller(method: "POST", params: { "authenticity_token" => masked }, session_token: secret).verified_request?
    assert controller(method: "POST", params: { "authenticity_token" => secret }, session_token: secret).verified_request?
    assert controller(method: "DELETE", headers: { "HTTP_X_CSRF_TOKEN" => masked }, session_token: secret).verified_request?
    c = controller(method: "PATCH", params: { "authenticity_token" => masked }, session_token: secret)
    c.verify_authenticity_token
    refute c.performed?
  end

  def test_a_wrong_token_or_a_session_without_one_is_refused
    secret = minted
    refute controller(method: "POST", params: { "authenticity_token" => secret.reverse }, session_token: secret).verified_request?
    refute controller(method: "POST", params: { "authenticity_token" => secret[0, 10] }, session_token: secret).verified_request?
    refute controller(method: "POST", params: { "authenticity_token" => "" }, session_token: nil).verified_request?
  end

  def test_a_foreign_or_null_origin_is_refused_even_with_the_token
    secret = minted
    ok = { "authenticity_token" => secret }
    assert controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "http://chat.example.com" }, session_token: secret).verified_request?
    refute controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "https://evil.example" }, session_token: secret).verified_request?
    refute controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "null" }, session_token: secret).verified_request?
  end

  # actionpack compares the Origin with `request.base_url`, scheme and
  # port included: the same host over the other scheme, or on another
  # port, is another origin.
  def test_the_origin_must_be_the_request_base_url_scheme_included
    secret = minted
    ok = { "authenticity_token" => secret }
    tls = { "HTTPS" => "on" }
    assert controller(method: "POST", params: ok, headers: tls.merge("HTTP_ORIGIN" => "https://chat.example.com"), session_token: secret).verified_request?
    refute controller(method: "POST", params: ok, headers: tls.merge("HTTP_ORIGIN" => "http://chat.example.com"), session_token: secret).verified_request?
    refute controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "https://chat.example.com" }, session_token: secret).verified_request?
    refute controller(method: "POST", params: ok, headers: { "HTTP_ORIGIN" => "http://chat.example.com:8080" }, session_token: secret).verified_request?
    rack_https = { "rack.url_scheme" => "https", "HTTP_ORIGIN" => "https://chat.example.com" }
    assert controller(method: "POST", params: ok, headers: rack_https, session_token: secret).verified_request?
    refute controller(method: "POST", params: ok, headers: rack_https.merge("HTTP_ORIGIN" => "http://chat.example.com"), session_token: secret).verified_request?
    proxied = { "HTTP_X_FORWARDED_PROTO" => "https", "HTTP_ORIGIN" => "https://chat.example.com" }
    assert controller(method: "POST", params: ok, headers: proxied, session_token: secret).verified_request?
    # A Host header naming the scheme's standard port is the same origin
    # a browser writes without it.
    explicit = tls.merge("HTTP_HOST" => "chat.example.com:443", "HTTP_ORIGIN" => "https://chat.example.com")
    assert controller(method: "POST", params: ok, headers: explicit, session_token: secret).verified_request?
  end

  # Action Cable's handshake check: same host, or any localhost port in
  # development; an absent Origin is refused, unlike the form check.
  def test_a_cable_handshake_must_come_from_its_own_host
    rfp = ActionController::RequestForgeryProtection
    base = rfp.base_url_for("chat.example.com:3000", "", "", "")
    assert rfp.cable_origin_allowed?("http://chat.example.com:3000", base, false)
    refute rfp.cable_origin_allowed?("https://chat.example.com:3000", base, false)
    refute rfp.cable_origin_allowed?("https://evil.example", base, false)
    refute rfp.cable_origin_allowed?("http://chat.example.com:4000", base, false)
    refute rfp.cable_origin_allowed?("", base, false)
    refute rfp.cable_origin_allowed?("null", base, false)
    tls = rfp.base_url_for("chat.example.com", "", "https", "")
    assert rfp.cable_origin_allowed?("https://chat.example.com", tls, false)
    refute rfp.cable_origin_allowed?("http://chat.example.com", tls, false)
    assert_equal "https://chat.example.com", rfp.base_url_for("chat.example.com", "on", "", "")
    assert_equal "https://chat.example.com", rfp.base_url_for("chat.example.com:443", "", "https", "")
    assert_equal "http://chat.example.com", rfp.base_url_for("chat.example.com:80", "", "", "")
    assert_equal "http://chat.example.com:443", rfp.base_url_for("chat.example.com:443", "", "", "")
    # Rack's own scheme counts when nothing else says TLS: a server that
    # sets only `rack.url_scheme` is still https. A proxy header wins.
    assert_equal "https://chat.example.com", rfp.base_url_for("chat.example.com:443", "", "", "https")
    assert_equal "http://chat.example.com", rfp.base_url_for("chat.example.com", "", "http", "https")
  end

  def test_development_also_allows_any_localhost_port
    rfp = ActionController::RequestForgeryProtection
    base = rfp.base_url_for("chat.example.com", "", "", "")
    refute rfp.cable_origin_allowed?("http://localhost:3000", base, false)
    assert rfp.cable_origin_allowed?("http://localhost:3000", base, true)
    assert rfp.cable_origin_allowed?("https://localhost:8443", base, true)
    refute rfp.cable_origin_allowed?("http://localhost:3000.evil.example", base, true)
    refute rfp.cable_origin_allowed?("", base, true)
  end

  # Rails main's Fetch Metadata rule, ahead of the token.
  # `Rails.application` is a fresh object per call, so the override
  # goes on the class (as ingest's lift does) and is put back after.
  def with_strategy(name)
    klass = Rails::Application
    original = klass.instance_method(:forgery_protection_verification_strategy)
    klass.define_method(:forgery_protection_verification_strategy) { name }
    yield
  ensure
    klass.define_method(:forgery_protection_verification_strategy, original)
  end

  def test_a_same_origin_or_same_site_header_passes_without_a_token
    %w[same-origin same-site SAME-ORIGIN].each do |site|
      assert controller(method: "POST", headers: { "HTTP_SEC_FETCH_SITE" => site }).verified_request?
    end
  end

  def test_a_cross_site_header_is_refused_even_with_the_token
    secret = minted
    refute controller(method: "POST", params: { "authenticity_token" => secret },
                      headers: { "HTTP_SEC_FETCH_SITE" => "cross-site" }, session_token: secret).verified_request?
  end

  def test_header_or_legacy_token_falls_back_to_the_token
    secret = minted
    refute controller(method: "POST", headers: { "HTTP_SEC_FETCH_SITE" => "none" }).verified_request?
    assert controller(method: "POST", params: { "authenticity_token" => secret },
                      headers: { "HTTP_SEC_FETCH_SITE" => "none" }, session_token: secret).verified_request?
  end

  def test_header_only_passes_a_missing_header_on_plain_http_only
    with_strategy("header_only") do
      assert controller(method: "POST").verified_request?
      refute controller(method: "POST", headers: { "HTTP_X_FORWARDED_PROTO" => "https" }).verified_request?
      secret = minted
      refute controller(method: "POST", params: { "authenticity_token" => secret },
                        headers: { "HTTP_SEC_FETCH_SITE" => "none" }, session_token: secret).verified_request?
    end
  end
end
