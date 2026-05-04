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
  # Re-loads fixtures after truncate so each test sees the canonical
  # rows; emitted `<Plural>Fixtures._fixtures_load!` methods carry the
  # YAML-derived `<Class>.new({...}).save` calls.
  def reset!
    TABLES.each { |t| ActiveRecord.adapter.truncate(t) }
    FixtureLoader.load_all!
  end
end

# Eager-load every emitted fixture file so each `*Fixtures` module is
# available regardless of which test file required which subset.
# Mirrors Rails's "all fixtures load on each test" convention — a test
# that destroys an article expects its associated comments to exist
# even when its `require_relative` block names only `articles`. The
# glob is intentionally cheap (a handful of files) and silent when the
# directory doesn't exist (e.g. for the standalone spinel-blog suite,
# which has no `test/fixtures/*.rb`).
fixtures_dir = File.join(ROOT, "test", "fixtures")
if File.directory?(fixtures_dir)
  Dir[File.join(fixtures_dir, "*.rb")].sort.each { |f| require f }
end

# Walks `Object.constants` for `*Fixtures` modules and dispatches their
# `_fixtures_load!` (emitted by `lower_fixtures_to_library_classes`).
# Discovery via constant scan keeps the fixture file shape free of any
# top-level registration call — the lowerer only emits inside the
# module body. Hand-written tests with no `*Fixtures` modules in scope
# get a no-op, so the standalone spinel-blog suite (which seeds inline)
# is unaffected.
module FixtureLoader
  module_function

  # Alphabetical sort approximates parent-before-child for the
  # Articles → Comments shape (belongs_to FK validation requires the
  # parent row to exist when the child saves). Topological ordering by
  # belongs_to graph is the principled fix; defer until a fixture set
  # exposes a non-alphabetic dependency.
  #
  # Filter by `*Fixtures` suffix BEFORE `const_get` so deprecated
  # constants like Ruby 3.4's `SortedSet` (which raises on access via
  # autoload) don't get touched while scanning for fixture modules.
  def load_all!
    Object.constants.sort.each do |c|
      next unless c.to_s.end_with?("Fixtures")
      mod = Object.const_get(c)
      next unless mod.is_a?(Module)
      next unless mod.respond_to?(:_fixtures_load!)
      mod._fixtures_load!
    end
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
  # Equivalent of Rails's transactional fixtures: every test starts
  # with a freshly-truncated DB plus the canonical fixture rows. Runs
  # before user `setup` (Minitest's documented `before_setup` →
  # `setup` → `after_setup` ordering). Hand-written tests that call
  # `SchemaSetup.reset!` themselves get a redundant-but-idempotent
  # second truncate; harmless.
  def before_setup
    super
    SchemaSetup.reset! if defined?(SchemaSetup)
  end

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

  # `assert_no_difference("Comment.count") { ... }` — companion of
  # assert_difference fixed at delta 0. Same single-expression form
  # the lowered tests use.
  def assert_no_difference(expression)
    before = eval(expression)
    yield
    after = eval(expression)
    assert_equal(before, after, "#{expression} changed (was #{before}, now #{after})")
  end
end

# `ActionDispatch::IntegrationTest` parent — Rails controller tests
# inherit from this. Define it as a Minitest::Test subclass that mixes
# in RequestDispatch so the emitted `class XControllerTest <
# ActionDispatch::IntegrationTest` resolves without an emit-time
# parent rewrite. Lives below RequestDispatch's definition (defined
# below) so the include resolves.
module RequestDispatch
  # Forward declaration — body defined below; placeholder lets
  # ActionDispatch::IntegrationTest's `include` reference resolve
  # without reordering the file. Ruby reopens the module when the
  # real definition lands.
end

module ActionDispatch
  class IntegrationTest < Minitest::Test
    include RequestDispatch
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
    @__response = ActionResponse.new(
      status:   controller.status,
      body:     controller.body,
      location: controller.location,
      flash:    controller.flash,
    )
    @__response
  end

  # Symbol-form HTTP-status assertion. Real-blog tests pass `:success`,
  # `:unprocessable_entity`, etc.; the table covers what real-blog
  # surfaces today. Numeric form (`assert_response 200`) and
  # range-form (`assert_response 200..299`) also work for parity with
  # ActionDispatch::IntegrationTest.
  STATUS_SYMBOLS = {
    success:              200..299,
    redirect:             300..399,
    missing:              404,
    not_found:            404,
    error:                500..599,
    ok:                   200,
    created:              201,
    no_content:           204,
    moved_permanently:    301,
    found:                302,
    see_other:            303,
    bad_request:          400,
    unauthorized:         401,
    forbidden:            403,
    unprocessable_entity: 422,
    internal_server_error: 500,
  }.freeze

  def assert_response(expected, response = @__response)
    actual = response.status
    expected_match = expected.is_a?(Symbol) ? STATUS_SYMBOLS[expected] : expected
    matches = case expected_match
              when Range   then expected_match.include?(actual)
              when Integer then expected_match == actual
              else false
              end
    assert matches,
      "expected response #{expected.inspect}, got status=#{actual} body=#{response.body[0, 200].inspect}"
  end

  # Two-argument form retained for hand-written spinel-blog tests
  # (`assert_redirected_to "/articles/1", res`); single-argument form
  # used by emitted tests pulls from the dispatch-stashed response.
  def assert_redirected_to(expected_path, response = @__response)
    assert response.redirect?,
      "expected a redirect, got status=#{response.status} location=#{response.location.inspect}"
    assert_equal expected_path, response.location
  end

  # Minimal `assert_select` shim — body-substring matching, NOT a real
  # CSS-selector engine. Two forms exercised by real-blog:
  #   assert_select("h1", "Articles")          → body matches /<h1[^>]*>\s*Articles\s*</
  #   assert_select("form")                    → body contains "<form"
  #   assert_select("#comments .p-4", minimum: 1) → fall through to
  #     id-substring + class-substring presence (no nesting verified)
  # Block form (`assert_select(parent) { … }`) ignores the parent
  # scope and runs the block against the same body — adequate for
  # real-blog's two block-form usages, both of which assert presence
  # rather than nested counts. Tighten if a fixture exposes a false
  # positive.
  def assert_select(selector, content_or_opts = nil, opts = nil, &block)
    body = @__response.body
    if content_or_opts.is_a?(Hash)
      opts = content_or_opts
      content = nil
    else
      content = content_or_opts
    end
    if content.is_a?(String)
      tag = selector[/\A[a-z]+/]
      pattern = if tag
                  Regexp.new("<#{tag}[^>]*>\\s*#{Regexp.escape(content)}\\s*<")
                else
                  Regexp.new(Regexp.escape(content))
                end
      assert pattern.match?(body),
        "expected #{selector.inspect} containing #{content.inspect} in response body"
    elsif selector.start_with?("#")
      id = selector.split(" ", 2).first[1..]
      assert body.include?(%(id="#{id}")),
        "expected element with id #{id.inspect} in response body"
    elsif selector.include?(".")
      _tag, cls = selector.split(".", 2)
      assert body.include?(%(class="#{cls})) || body.match?(/class="[^"]*\b#{Regexp.escape(cls)}\b/),
        "expected element with class #{cls.inspect} in response body"
    else
      tag = selector[/\A[a-z]+/]
      assert tag && body.include?("<#{tag}"),
        "expected #{selector.inspect} in response body"
    end
    yield if block
  end
end
