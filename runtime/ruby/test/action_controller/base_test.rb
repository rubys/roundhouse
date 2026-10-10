require_relative "../test_helper"

# Direct unit tests for `runtime/ruby/action_controller/base.rb`.
# Exercises the controller-state surface (status / body / location /
# flash) through a TestController subclass that supplies the
# `process_action` override Base requires.
class ActionControllerBaseTest < Minitest::Test
  # Smallest subclass that satisfies Base's contract — process_action
  # dispatches by name to the corresponding action method.
  class TestController < ActionController::Base
    def process_action(action_name)
      # Explicit `()` on each dispatch — Ruby treats parenless
      # `index` as a method call, but the TS emit defaults to
      # property-read for instance-receiver zero-arg sends (the
      # body-typer's AccessorKind doesn't yet thread through to
      # Send emit). Parens force the call shape on both sides.
      case action_name.to_sym
      when :index then index()
      when :create then create()
      when :destroy then destroy()
      end
    end

    def index
      render "<h1>Hello</h1>"
    end

    def create
      redirect_to "/articles/1", notice: "Created", status: :see_other
    end

    def destroy
      head :no_content
    end
  end

  def setup
    @controller = TestController.new
  end

  # ── initialization defaults ──────────────────────────────────

  def test_initial_state_has_empty_params_and_default_status
    refute_nil @controller.params
    assert_empty @controller.params
    assert_equal 0, @controller.session.length()
    assert_equal 0, @controller.flash.length()
    assert_equal 200, @controller.status
    assert_equal "", @controller.body
    assert_nil @controller.location
  end

  # ── render ──────────────────────────────────────────────────

  def test_render_sets_body_and_default_200_status
    @controller.process_action(:index)
    assert_equal "<h1>Hello</h1>", @controller.body
    assert_equal 200, @controller.status
  end

  def test_response_content_type_can_be_assigned
    @controller.response.content_type = "text/vnd.turbo-stream.html"
    assert_equal "text/vnd.turbo-stream.html", @controller.content_type
  end

  def test_media_type_drops_the_content_type_parameters
    @controller.response.content_type = "text/html; charset=utf-8"
    assert_equal "text/html", @controller.media_type
  end

  def test_response_body_can_be_assigned
    @controller.response.response_body = "<p>body</p>"
    assert_equal "<p>body</p>", @controller.body
    assert_equal "<p>body</p>", @controller.response_body
    assert @controller.performed?
  end

  def test_nil_response_body_clears_the_body_without_performing
    @controller.response.response_body = nil
    assert_equal "", @controller.body
    refute @controller.performed?
  end

  # `render(..., status: 422)` (Integer literal) is no longer part of the
  # public API. `status:` is monomorphic Symbol — callers needing an
  # explicit integer code coerce at the call site. The contraction
  # gives every backend compiler a stable input shape (see
  # project_compilers_were_ready.md for the design rationale).

  def test_render_accepts_symbolic_status
    @controller.render("err", status: :unprocessable_entity)
    assert_equal 422, @controller.status
  end

  # ── redirect_to ─────────────────────────────────────────────

  def test_redirect_to_sets_location_and_status
    @controller.process_action(:create)
    assert_equal "/articles/1", @controller.location
    # :see_other → 303
    assert_equal 303, @controller.status
  end

  def test_redirect_to_default_status_is_found_302
    @controller.redirect_to("/somewhere")
    assert_equal 302, @controller.status
    assert_equal "/somewhere", @controller.location
  end

  def test_redirect_to_propagates_notice_to_flash
    @controller.redirect_to("/x", notice: "Saved")
    assert_equal "Saved", @controller.flash.fetch(:notice)
    refute @controller.flash.key?(:alert)
  end

  def test_redirect_to_propagates_alert_to_flash
    @controller.redirect_to("/x", alert: "Bad")
    assert_equal "Bad", @controller.flash.fetch(:alert)
  end

  def test_redirect_to_omits_flash_keys_when_nil
    @controller.redirect_to("/x")
    assert_empty @controller.flash
  end

  # ── head ────────────────────────────────────────────────────

  def test_head_on_an_unperformed_controller_sets_status_and_empty_body
    assert_equal true, @controller.head(:no_content)
    assert_equal 204, @controller.status
    assert_equal "", @controller.body
    assert @controller.performed?
  end

  def test_head_options_set_normalized_headers_location_and_content_type
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

  def test_head_resolves_mime_symbol_content_type
    @controller.head(:ok, { content_type: :json })
    assert_equal "application/json", @controller.content_type
  end

  def test_head_invalid_mime_symbol_does_not_mutate_controller_or_options
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

  def test_head_defaults_nil_status_to_ok
    assert_equal true, @controller.head(nil)
    assert_equal 200, @controller.status
    assert_equal "", @controller.body
    assert_equal "text/html", @controller.content_type
  end

  def test_head_preserves_a_preexisting_media_type
    @controller.content_type = "application/problem+json; charset=utf-8"
    @controller.head(:ok, { content_type: "text/plain" })
    assert_equal "application/problem+json", @controller.content_type
  end

  def test_head_defaults_when_the_preexisting_media_type_is_empty
    @controller.content_type = ""
    @controller.head(:ok)
    assert_equal "text/html", @controller.content_type
  end

  def test_head_uses_the_registered_mime_for_the_negotiated_format
    @controller.request_format = :xml
    @controller.head(:ok)
    assert_equal "application/xml", @controller.content_type
  end

  def test_head_accepts_integer_statuses
    assert_equal true, @controller.head(418)
    assert_equal 418, @controller.status
  end

  def test_head_omits_content_type_for_all_bodyless_status_classes
    [100, 199, 204, 205, 304].each do |status|
      controller = ActionController::Base.new
      controller.head(status)
      assert_equal "", controller.content_type, "status #{status}"
      assert_equal "", controller.body, "status #{status}"
    end
  end

  def test_head_rejects_hash_status
    error = assert_raises(ArgumentError) { @controller.head(location: "/wrong") }
    assert_includes error.message, "not a valid value for `status`"
    refute @controller.performed?
  end

  def test_head_raises_double_render_after_render_without_replacing_body
    @controller.render("partial output")
    assert_equal "partial output", @controller.body

    assert_raises(AbstractController::DoubleRenderError) { @controller.head(:no_content) }
    assert_equal 200, @controller.status
    assert_equal "partial output", @controller.body
  end

  def test_head_rejects_status_symbols_outside_the_registry
    assert_raises(ArgumentError) { @controller.head(:not_a_status) }
    refute @controller.performed?
  end

  # ── resolve_status ──────────────────────────────────────────

  # The Integer pass-through case (`resolve_status(418)`) is removed
  # along with the `untyped` parameter; the contract is Symbol-only.

  def test_resolve_status_maps_known_symbols
    assert_equal 200, @controller.resolve_status(:ok)
    assert_equal 201, @controller.resolve_status(:created)
    assert_equal 303, @controller.resolve_status(:see_other)
    assert_equal 404, @controller.resolve_status(:not_found)
    assert_equal 422, @controller.resolve_status(:unprocessable_entity)
  end

  # The whole Rack registry, not the handful real-blog happens to use.
  # `:too_many_requests` is here because campfire's rate-limit filter
  # asked for it and got a 200 — the miss is what made the fallback
  # below a hazard rather than a convenience.
  def test_resolve_status_covers_the_full_registry
    assert_equal 429, @controller.resolve_status(:too_many_requests)
    assert_equal 503, @controller.resolve_status(:service_unavailable)
    assert_equal 451, @controller.resolve_status(:unavailable_for_legal_reasons)
    assert_equal 100, @controller.resolve_status(:continue)
  end

  def test_resolve_status_raises_on_unknown_symbol
    assert_raises(RuntimeError) { @controller.resolve_status(:totally_invented) }
  end

  # ── process_action ──────────────────────────────────────────

  def test_base_process_action_raises_when_not_overridden
    bare = ActionController::Base.new
    assert_raises(NotImplementedError) { bare.process_action(:anything) }
  end

  def test_subclass_process_action_dispatches_to_named_action
    @controller.process_action(:destroy)
    assert_equal 204, @controller.status
    assert_equal "", @controller.body
  end

  # ── STATUS_CODES surface ────────────────────────────────────
  # Originally probed `ActionController::STATUS_CODES` directly
  # (`.key?(sym)`, `:frozen?`). The constant is internal to the
  # framework runtime and not exported across targets; the symbol
  # → code mapping is observable through `resolve_status`, which
  # is the public API every target preserves. Spirit survives via
  # the indirection — drift in either the constant or the resolver
  # surfaces as a wrong code coming back from `resolve_status`.

  def test_resolve_status_covers_every_symbol_used_in_real_blog
    expectations = {
      ok: 200, created: 201, no_content: 204, see_other: 303,
      found: 302, not_found: 404, unprocessable_entity: 422,
    }
    expectations.each do |sym, code|
      assert_equal code, @controller.resolve_status(sym),
        "resolve_status mismapped :#{sym}"
    end
  end
end
