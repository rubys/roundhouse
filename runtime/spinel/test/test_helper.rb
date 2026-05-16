require "minitest/autorun"

# Copied verbatim to <out>/test/test_helper.rb (by `make ruby-transpile`
# or `tests/ruby_toolchain.rs`). `__dir__` is `<out>/test/`, so the
# `require_relative` paths walk up one level to reach `runtime/`, `config/`,
# and `test/fixtures/`. `require_relative` (not bare `require` + LOAD_PATH)
# is mandatory because spinel's AOT model only follows static
# `require_relative` chains — bare `require` with `$LOAD_PATH` lookup is a
# CRuby-only mechanism that the AOT compiler cannot resolve.
#
# The Ruby-target tree contains a single `runtime/db.rb` (gem-backed,
# materialized from `runtime/spinel/db_cruby.rb` at transpile time);
# the future Spinel-AOT target's tree will contain its own
# `runtime/db.rb` (FFI-backed). Same require path, target-appropriate
# implementation.
require_relative "../runtime/base64"
require_relative "../runtime/json"
require_relative "../runtime/db"
require_relative "../runtime/active_record"
require_relative "../config/schema"
require_relative "../runtime/action_dispatch"
require_relative "../runtime/action_controller"
require_relative "../runtime/action_view"
require_relative "../runtime/json_builder"
require_relative "../runtime/broadcasts"
require_relative "../runtime/importmap"
require_relative "../config/importmap"
require_relative "../config/routes"

# One-time global setup: configure the Db primitive surface (cruby
# shim under stock CRuby — `runtime/spinel/db.rb` wraps the sqlite3
# gem; FFI shim under spinel-compiled binaries once matz/spinel#405
# lands), load the schema via Db.exec, and rely on lowerer-emitted
# per-model `_adapter_*` Level-3 primitives for all AR access. The
# 12-method `ActiveRecord.adapter` shape is intentionally NOT wired —
# any path that falls through to it surfaces a NoMethodError on nil
# and tells us which AR call needs Level-3 emit next.
#
# Per-test isolation comes from `SchemaSetup.reset!` calling each
# model's `_adapter_truncate`. Each model's lowered class has its
# own truncate primitive (per-table DELETE).
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }

module SchemaSetup
  # Per-model truncate via lowerer-emitted `_adapter_truncate`. The
  # constant list is the same as before; flipping each table's
  # truncate call from `ActiveRecord.adapter.truncate(t)` to
  # `<Model>._adapter_truncate` is the per-model dispatch.
  def self.reset!
    Article._adapter_truncate if defined?(Article)
    Comment._adapter_truncate if defined?(Comment)
    FixtureLoader.load_all!
  end
end

# Fixture files are loaded via explicit `require_relative` lines
# injected into each test file's preamble by `src/emit/ruby.rs`
# (which is required under spinel AOT, where dynamic `Dir[…]` + `require`
# isn't available). The previous CRuby-only Dir-glob fallback was
# removed — emit always injects explicit requires so the fallback was
# always dead in practice, and the dynamic-method block produced
# spurious "emitting 0" warnings under spinel.

# Walks `Object.constants` for `*Fixtures` modules and dispatches their
# `_fixtures_load!` (emitted by `lower_fixtures_to_library_classes`).
# Discovery via constant scan keeps the fixture file shape free of any
# top-level registration call — the lowerer only emits inside the
# module body. Hand-written tests with no `*Fixtures` modules in scope
# get a no-op, so the standalone spinel-blog suite (which seeds inline)
# is unaffected.
module FixtureLoader
  # Alphabetical sort approximates parent-before-child for the
  # Articles → Comments shape (belongs_to FK validation requires the
  # parent row to exist when the child saves). Topological ordering by
  # belongs_to graph is the principled fix; defer until a fixture set
  # exposes a non-alphabetic dependency.
  #
  # The `Object.constants` + `const_get` scan below is rewritten by
  # `src/emit/ruby.rs::render_test_helper` into explicit
  # `<X>Fixtures._fixtures_load!` calls per emitted spinel project.
  # Spinel's AOT model rejects `Object.constants` and `Object.const_get`
  # (no runtime constant table); the rewrite keeps the source-side
  # framework_ruby_tests_pass gate working under stock CRuby while
  # giving emitted projects a subset-clean equivalent.
  #
  # Filter by `*Fixtures` suffix BEFORE `const_get` so deprecated
  # constants like Ruby 3.4's `SortedSet` (which raises on access via
  # autoload) don't get touched while scanning for fixture modules.
  def self.load_all!
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
  # Bring `ActionView::ViewHelpers` and `ActionDispatch::Router` into
  # scope as bare `ViewHelpers` / `Router` for the request-dispatch
  # body — matches Ruby's `include` idiom for nested-module access.
  include ActionView
  include ActionDispatch

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
    require_relative "../config/routes"
    ViewHelpers.reset_slots!
    matched = Router.match(method, path, Routes.table)
    raise "No route matches #{method} #{path}" if matched.nil?
    controller = case matched.controller
                 when :articles then ArticlesController.new
                 when :comments then CommentsController.new
                 end
    merged = matched.path_params.dup
    # Test fixtures pass Symbol-keyed nested hashes (`{article: {title:
    # ...}}`); the wire-level request body is String-keyed at runtime.
    # Stringify recursively so the harness shape matches what the
    # request-body parser would produce in production.
    params.each { |k, v| merged[k.to_s] = stringify_keys(v) }
    controller.params  = merged
    controller.session = @__session ||= ActionDispatch::Session.new
    controller.flash   = @__flash   ||= ActionDispatch::Flash.new
    controller.request_method = method
    controller.request_path   = path
    controller.process_action(matched.action)
    @__flash = controller.flash
    @__response = ActionResponse.new(
      status:   controller.status,
      body:     controller.body,
      location: controller.location,
      flash:    controller.flash,
    )
    @__response
  end

  # Recursively stringify Hash keys. Test fixtures pass Symbol-keyed
  # nested hashes (Ruby's idiomatic shape); the wire-level request
  # body parser would produce String keys. Used to normalize at the
  # harness boundary so @params has the production shape.
  def stringify_keys(value)
    return value unless value.is_a?(Hash)
    out = {}
    value.each { |k, v| out[k.to_s] = stringify_keys(v) }
    out
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
    # Rails 8.1.x scaffold renamed `:unprocessable_entity` →
    # `:unprocessable_content` mid-version (HTTP 422 description
    # churn). Alias both so test asserts work regardless of which
    # the fixture's scaffold currently produces.
    unprocessable_content: 422,
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
