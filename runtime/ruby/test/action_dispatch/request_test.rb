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
end
