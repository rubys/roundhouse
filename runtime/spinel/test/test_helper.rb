require "minitest/autorun"

# ROOT is the project root. Two callers, two locations:
# 1. Overlay flow: this file is copied verbatim to <out>/test/test_helper.rb
#    (by `make spinel-transpile` or `tests/spinel_toolchain.rs`) — `__dir__`
#    is `<out>/test/`, so `..` gives `<out>/`. Correct.
# 2. Standalone flow: this file is loaded via the bridge at
#    fixtures/spinel-blog/test/test_helper.rb, whose `__dir__` is the
#    fixture's `test/`. The bridge sets ROOT before require_relative; this
#    guard preserves the bridge's value instead of recomputing from the
#    canonical's `__dir__` (which would point at `runtime/spinel/`).
ROOT = File.expand_path("..", __dir__) unless defined?(ROOT)
$LOAD_PATH.unshift(File.join(ROOT, "runtime"))
$LOAD_PATH.unshift(File.join(ROOT, "app"))
$LOAD_PATH.unshift(File.join(ROOT, "config"))

require "sqlite_adapter"
require "active_record"
require "schema"
require "action_dispatch"
require "action_controller"

# One-time global setup: configure the adapter against an in-memory
# SQLite database, load the schema, and wire ActiveRecord.adapter to
# point at it. Per-test isolation comes from `SchemaSetup.reset!` —
# called from each test class's `setup` block — which truncates the
# tables but leaves the schema intact.
SqliteAdapter.configure(":memory:")
Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }
ActiveRecord.adapter = SqliteAdapter

module SchemaSetup
  module_function

  TABLES = %w[articles comments].freeze

  # Adapter-agnostic: dispatches through ActiveRecord.adapter.truncate
  # so tests work whether the adapter is SqliteAdapter or
  # InMemoryAdapter (or any future adapter conforming to the API).
  def reset!
    TABLES.each { |t| ActiveRecord.adapter.truncate(t) }
  end
end

# In-process request dispatch — equivalent of Rails's
# ActionDispatch::IntegrationTest. Test classes that need to exercise
# controller actions extend this module to get get/post/patch/delete.
class ActionResponse
  attr_reader :status, :body, :location, :flash

  def initialize(status:, body:, location:, flash:)
    @status   = status
    @body     = body
    @location = location
    @flash    = flash
  end

  def redirect?
    !@location.nil? && @status >= 300 && @status < 400
  end

  def success?
    @status >= 200 && @status < 300
  end

  def unprocessable?
    @status == 422
  end
end

# ActiveSupport::TestCase compatibility shims — emitted tests inherit
# from Minitest::Test (rewritten at emit time from `ActiveSupport::
# TestCase`) but call AS-extension assertion methods. Reopen Minitest::
# Test so every test class picks them up without a parent change.
class Minitest::Test
  def assert_not(value, msg = nil)
    refute(value, msg)
  end

  def assert_not_nil(value, msg = nil)
    refute_nil(value, msg)
  end

  # Evaluates `expression` (a String containing Ruby code) before and
  # after `block`, asserting the integer delta matches `change`. Mirror
  # of ActiveSupport::Testing::Assertions#assert_difference for the
  # single-expression case the lowered tests use.
  def assert_difference(expression, change = 1)
    before = eval(expression)
    yield
    after = eval(expression)
    assert_equal(before + change, after,
      "#{expression} didn't change by #{change}")
  end
end

module RequestDispatch
  def get(path, params: {})
    dispatch_request("GET", path, params)
  end

  def post(path, params: {})
    dispatch_request("POST", path, params)
  end

  def patch(path, params: {})
    dispatch_request("PATCH", path, params)
  end

  def delete(path, params: {})
    dispatch_request("DELETE", path, params)
  end

  def dispatch_request(method, path, params)
    require "routes"
    ViewHelpers.reset_slots!
    matched = Router.match(method, path, Routes.table)
    raise "No route matches #{method} #{path}" if matched.nil?
    controller = case matched[:controller]
                 when :articles then ArticlesController.new
                 when :comments then CommentsController.new
                 end
    merged = matched[:path_params].dup
    params.each { |k, v| merged[k] = v }
    controller.params  = ActionController::Parameters.new(merged)
    controller.session = @__session ||= {}
    controller.flash   = @__flash   ||= {}
    controller.request_method = method
    controller.request_path   = path
    controller.process_action(matched[:action])
    @__flash = controller.flash
    ActionResponse.new(
      status:   controller.status,
      body:     controller.body,
      location: controller.location,
      flash:    controller.flash,
    )
  end

  def assert_redirected_to(expected_path, response)
    assert response.redirect?,
      "expected a redirect, got status=#{response.status} location=#{response.location.inspect}"
    assert_equal expected_path, response.location
  end
end
