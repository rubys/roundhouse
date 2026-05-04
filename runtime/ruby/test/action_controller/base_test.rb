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
      case action_name.to_sym
      when :index then index
      when :create then create
      when :destroy then destroy
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
    assert_equal({}, @controller.params.to_h)
    assert_equal({}, @controller.session)
    assert_equal({}, @controller.flash)
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

  def test_render_accepts_explicit_integer_status
    @controller.render("err", status: 422)
    assert_equal 422, @controller.status
  end

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
    assert_equal "Saved", @controller.flash[:notice]
    refute @controller.flash.key?(:alert)
  end

  def test_redirect_to_propagates_alert_to_flash
    @controller.redirect_to("/x", alert: "Bad")
    assert_equal "Bad", @controller.flash[:alert]
  end

  def test_redirect_to_omits_flash_keys_when_nil
    @controller.redirect_to("/x")
    assert_empty @controller.flash
  end

  # ── head ────────────────────────────────────────────────────

  def test_head_sets_status_and_clears_body
    @controller.body = "leftover" if @controller.respond_to?(:body=)
    # body= isn't an attr_writer (only reader); set via render to
    # populate, then head must clear.
    @controller.render("partial output")
    assert_equal "partial output", @controller.body

    @controller.head(:no_content)
    assert_equal 204, @controller.status
    assert_equal "", @controller.body
  end

  def test_head_accepts_integer_status
    @controller.head(404)
    assert_equal 404, @controller.status
  end

  # ── resolve_status ──────────────────────────────────────────

  def test_resolve_status_passes_integer_through
    assert_equal 418, @controller.resolve_status(418)
  end

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
  # The constant is consumed across every target (TS prelude, etc.);
  # asserting its shape catches drift between framework Ruby and
  # any per-target hand-mirror.

  def test_status_codes_covers_every_symbol_used_in_real_blog
    # Subset real-blog actions actually pass to render/redirect_to/head.
    %i[ok created no_content see_other found not_found unprocessable_entity]
      .each do |sym|
      assert ActionController::STATUS_CODES.key?(sym),
        "STATUS_CODES missing :#{sym}"
    end
  end

  def test_status_codes_is_frozen
    assert_predicate ActionController::STATUS_CODES, :frozen?
  end
end
