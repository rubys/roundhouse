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

  def test_head_sets_status_and_clears_body
    # body= isn't an attr_writer (only reader); populate via render,
    # then head must clear. (The original test guarded a pre-set with
    # `respond_to?(:body=)` — a no-op since the writer doesn't exist
    # — and TS has no respond_to?, so drop the guard.)
    @controller.render("partial output")
    assert_equal "partial output", @controller.body

    @controller.head(:no_content)
    assert_equal 204, @controller.status
    assert_equal "", @controller.body
  end

  # `head(404)` (Integer literal) — same contraction as render's status:
  # parameter. Symbol-only now.

  # ── resolve_status ──────────────────────────────────────────

  # The Integer pass-through case (`resolve_status(418)`) is removed
  # along with the `untyped` parameter; the Symbol-only contract makes
  # `STATUS_CODES.fetch(s, 200)` the entire body.

  def test_resolve_status_maps_known_symbols
    assert_equal 200, @controller.resolve_status(:ok)
    assert_equal 201, @controller.resolve_status(:created)
    assert_equal 303, @controller.resolve_status(:see_other)
    assert_equal 404, @controller.resolve_status(:not_found)
    assert_equal 422, @controller.resolve_status(:unprocessable_entity)
  end

  def test_resolve_status_falls_back_to_200_on_unknown_symbol
    assert_equal 200, @controller.resolve_status(:totally_invented)
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
