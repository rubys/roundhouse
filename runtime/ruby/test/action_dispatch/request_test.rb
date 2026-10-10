require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_dispatch/request.rb`.
#
# THE POINT OF THIS FILE IS THE CONSTRUCTOR. `Request.for` COPIES into
# `@env` and `@params` rather than assigning them (a caller's narrow
# `Hash[String, String]` literal is a real type error against the wide
# declared slot), which means it READS both before writing. An ivar the
# .rbs declares and `initialize` forgets is nil on a dynamic target and
# a null pointer under spinel AOT — `sp_StrPolyHash_set` dereferenced
# one and the whole test binary segfaulted with no output at all.
#
# It stayed invisible because the two ruby-family trees run DIFFERENT
# Request classes: the CRuby lane loads the overlay twin
# (`runtime/action_dispatch_request.rb`), so `ruby_toolchain` passed the
# same test file `spinel_toolchain` died on. This file exercises THIS
# class on both.
class ActionDispatchRequestTest < Minitest::Test
  def test_every_declared_ivar_has_a_value_after_new
    r = ActionDispatch::Request.new
    # Names from request.rbs's ivar block. A new one added there
    # without a value here is what this test is for.
    %i[
      @remote_ip @path @query_string @script_name @request_method
      @referer @host @format @body @env @user_agent @params
    ].each do |name|
      assert !r.instance_variable_get(name).nil?,
             "#{name} is unset after `new` — `Request.for` reads it before writing"
    end
  end

  def test_for_copies_env_and_params_rather_than_assigning
    r = ActionDispatch::Request.for({ "PATH_INFO" => "/articles" }, { "id" => "7" })
    assert_equal "/articles", r.path
    assert_equal "7", r.params["id"]
    assert_equal "/articles", r.env["PATH_INFO"]
  end

  # The default `params` is what the harness's no-params calls pass;
  # it must still leave a usable Hash behind.
  def test_for_without_params_leaves_an_empty_hash
    r = ActionDispatch::Request.for({ "REQUEST_METHOD" => "POST" })
    assert_equal({}, r.params)
    assert_equal "POST", r.request_method
  end

  # A key missing from env falls back to the documented default, and a
  # present one is coerced to String (env holds `untyped` by contract).
  def test_missing_env_keys_take_their_defaults
    r = ActionDispatch::Request.for({})
    assert_equal "GET", r.request_method
    assert_equal "/", r.path
    assert_equal "localhost", r.host
    assert_equal "127.0.0.1", r.remote_ip
  end

  def test_headers_fetch_distinguishes_missing_default_from_nil_default
    headers = ActionDispatch::Http::Headers.new({})
    assert_raises(KeyError) { headers.fetch("X-Required") }
    assert_nil headers.fetch("X-Optional", nil)
    assert_equal "fallback", headers.fetch("X-Optional", "fallback")
  end

  # The scheme every absolute URL is built with. Behind a proxy that
  # terminated TLS (Fly, a load balancer) the connection is plain http
  # and only `X-Forwarded-Proto` says the page is https; answering http
  # there made `room_refresh_url` mixed content and the browser blocked
  # it. Rack honors the header unconfigured, so Rails does too.
  def test_the_scheme_is_https_behind_a_tls_terminating_proxy
    r = ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test", "HTTP_X_FORWARDED_PROTO" => "https" })
    assert r.ssl?
    assert_equal "https://", r.protocol
    assert_equal "https://chat.test", r.base_url
  end

  def test_the_scheme_is_http_without_tls_or_a_proxy_header
    r = ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test" })
    assert !r.ssl?
    assert_equal "http://", r.protocol
    assert_equal "http://chat.test", r.base_url
  end

  def test_headers_accept_case_insensitive_http_and_rack_names
    r = ActionDispatch::Request.for({
      "HTTP_X_CAMPFIRE_BOT_KEY" => "bot-secret",
      "CONTENT_TYPE" => "application/json",
    })
    assert_equal "bot-secret", r.headers["X-Campfire-Bot-Key"]
    assert_equal "bot-secret", r.headers["http_x_campfire_bot_key"]
    assert_equal "bot-secret", r.headers.fetch("HtTp_X_CaMpFiRe_BoT_KeY")
    assert_equal "application/json", r.headers.fetch("content-type", "missing")
    assert_equal "application/json", r.headers["content_type"]
    assert r.headers.key?("x-campfire-bot-key")
    assert !r.headers.key?("X-Missing")
  end

  def test_authorization_checks_rack_and_legacy_env_keys
    assert_equal "Bearer key", ActionDispatch::Request.for(
      { "HTTP_AUTHORIZATION" => "Bearer key" }
    ).authorization
    assert_equal "Bearer legacy", ActionDispatch::Request.for(
      { "X-HTTP-AUTHORIZATION" => "Bearer legacy" }
    ).authorization
    assert_equal "Bearer redirect", ActionDispatch::Request.for(
      { "REDIRECT_X_HTTP_AUTHORIZATION" => "Bearer redirect" }
    ).authorization
  end

  def test_path_parameters_are_route_only_and_indifferent
    r = ActionDispatch::Request.new
    r.path_parameters = { "bot_key" => "route-secret" }
    assert_equal "route-secret", r.path_parameters[:bot_key]
    assert_equal "route-secret", r.path_parameters.fetch("bot_key", "missing")
    assert !r.path_parameters.key?(:query_key)
    assert_nil r.path_parameters.fetch(:query_key, nil)
    assert_raises(KeyError) { r.path_parameters.fetch(:query_key) }
  end

  # A proxy chain lists one scheme per hop; the first is the client's.
  def test_optional_port_is_nil_at_the_schemes_standard_port
    assert_nil ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test" }).optional_port
    assert_nil ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test:80" }).optional_port
    assert_nil ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test:443", "HTTPS" => "on" }).optional_port
    assert_nil ActionDispatch::Request.for({ "HTTP_HOST" => "[::1]" }).optional_port
  end

  def test_optional_port_is_any_other_port
    assert_equal 3000, ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test:3000" }).optional_port
    assert_equal 443, ActionDispatch::Request.for({ "HTTP_HOST" => "chat.test:443" }).optional_port
    assert_equal 8080, ActionDispatch::Request.for({ "HTTP_HOST" => "[::1]:8080" }).optional_port
  end

  def test_the_first_forwarded_scheme_is_the_clients
    assert ActionDispatch::Request.for({ "HTTP_X_FORWARDED_PROTO" => "https, http" }).ssl?
    assert ActionDispatch::Request.for({ "HTTPS" => "on" }).ssl?
  end

  # Rack requires `rack.url_scheme`; the other TLS keys are optional. A
  # proxy header, when present, still outranks it, as in Rack::Request.
  def test_rack_url_scheme_is_the_last_word_on_tls
    assert ActionDispatch::Request.for({ "rack.url_scheme" => "https" }).ssl?
    refute ActionDispatch::Request.for({ "rack.url_scheme" => "http" }).ssl?
    refute ActionDispatch::Request.for({}).ssl?
    refute ActionDispatch::Request.for({ "rack.url_scheme" => "https", "HTTP_X_FORWARDED_PROTO" => "http" }).ssl?
  end
end
