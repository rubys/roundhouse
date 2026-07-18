# Copied verbatim to <out>/test/test_helper.rb (by `make ruby-transpile`
# or `tests/ruby_toolchain.rs`). `__dir__` is `<out>/test/`, so the
# `require_relative` paths walk up one level to reach `runtime/`, `config/`,
# and `test/fixtures/`. `require_relative` (not bare `require` + LOAD_PATH)
# is mandatory because spinel's AOT model only follows static
# `require_relative` chains — bare `require` with `$LOAD_PATH` lookup is a
# CRuby-only mechanism that the AOT compiler cannot resolve.
#
# No `require "minitest/*"` — emitted tests inherit from
# `TestBase` (defined below), not `Minitest::Test`. Every test file
# ends with an explicit per-test driver shim (see
# emit/ruby.rs::render_autorun_shim) so there's nothing to autorun.
# Independent of Minitest entirely: insulates the emit from matz-
# analyzer changes around how spinel infers the Minitest::Test reopen
# (the original fragility motivating this rewrite), and frees CRuby
# runs from Minitest's `Minitest::Test#initialize(name)` argument
# expectation that the shim's zero-arg `.new` can't satisfy.
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
require_relative "../runtime/sqlite_adapter"
require_relative "../config/schema"
require_relative "../runtime/action_dispatch"
require_relative "../runtime/action_controller"
require_relative "../runtime/action_view"
require_relative "../runtime/json_builder"
require_relative "../runtime/broadcasts"
require_relative "../runtime/importmap"
require_relative "../config/importmap"
require_relative "../config/routes"
# The app/models.rb aggregator (generated — see apply_models_aggregator)
# loads every model/support class. Model files only require their own
# LOAD-time deps; tests reach the rest (and fixtures reach their models)
# through this line, mirroring main.rb's boot order.
require_relative "../app/models"

# One-time global setup: configure the Db primitive surface (cruby
# shim under stock CRuby — `runtime/spinel/db.rb` wraps the sqlite3
# gem; FFI shim under spinel-compiled binaries once matz/spinel#405
# lands), load the schema via Db.exec, and wire `ActiveRecord.adapter`
# to `SqliteAdapter`.
#
# Wiring the adapter matches the per-target test harnesses (crystal
# `test_helper.cr`, typescript) and the blog's `main.rb`. The generic AR
# class methods (`count`/`exists?`/`where`) delegate to
# `ActiveRecord.adapter` rather than per-model `_adapter_*` primitives, so
# their base bodies must type-check against a real adapter under spinel
# AOT — which compiles every method, including ones a per-model override
# shadows. CRuby tolerated an unwired (nil) adapter because it never
# compiles a dead base method; spinel does, and `ActiveRecord.adapter`
# resolved to its nil default makes `_adapter_count` emit a `nil` from an
# `Integer`-typed function (incompatible-pointer C error). SqliteAdapter
# shares the single Db configured here, so no separate `.configure` is
# needed.
#
# Per-test isolation comes from `SchemaSetup.reset!` calling each
# model's `_adapter_truncate`. Each model's lowered class has its
# own truncate primitive (per-table DELETE).
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter

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

# ── Dom primitive surface (the assert_select substrate) ────────────
#
# The HTML-query contract `assert_select` lowers to, shared in shape
# across every target (Ruby/TS/Python/Rust/Elixir/… — see the cross-
# target contract in runtime/spinel/test/test_helper.rbs). This is the
# historical substring matcher dressed as a Dom: `select` fabricates
# one synthetic node — the whole document body — per fragment
# occurrence, and `text` returns that node verbatim. So presence,
# `minimum:`, and content checks all degrade to exactly the pre-
# contract behavior. The upgrade path is to swap these three methods
# for a Nokogiri-backed (CRuby) / lexbor-FFI (spinel-AOT) engine —
# real nodes, real CSS selectors — touching only this module; the
# assert_select call site and every other target stay put.
#
# `parse`/`select`/`text` take/return Strings in the stub (doc and node
# are both "the html"). A real engine keeps the same method set but
# returns opaque tree/node handles — the contract is the surface, not
# the handle shape.
module Dom
  # Parse an HTML document. Stub: the document *is* its html string.
  def self.parse(html)
    html
  end

  # Nodes matching `selector` within `root` (a document or a node).
  # Stub: one synthetic node (the root's html) per substring-fragment
  # occurrence, so nested selects re-scan the whole string (the
  # historical no-scoping block behavior).
  def self.select(root, selector)
    fragment = fragment_for(selector)
    nodes = []
    from = 0
    while (i = root.index(fragment, from))
      nodes << root
      from = i + fragment.length
    end
    nodes
  end

  # Concatenated descendant text of a node. Stub: the node's html
  # verbatim (so a content check degrades to a body-substring check).
  def self.text(node)
    node
  end

  # Loose selector → substring fragment (the pre-contract rule):
  #   "#id"  → 'id="id"'   ".cls" → 'cls"'   "tag" → "<tag"
  # Compound selectors take the first whitespace chunk.
  def self.fragment_for(selector)
    first = selector.split(" ").first || ""
    if first.start_with?("#")
      %(id="#{first[1..]}")
    elsif first.start_with?(".")
      %(#{first[1..]}")
    else
      "<#{first}"
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

# Base class for every emitted test. Roundhouse-owned, no Minitest
# dependency. The Rails `class XTest < ActiveSupport::TestCase` form
# is rewritten at emit time (see src/emit/ruby.rs) so emitted tests
# inherit from TestBase directly. Provides the no-op lifecycle hooks
# the shim calls (`setup` / `teardown`) plus the per-test DB reset
# (`SchemaSetup.reset!` if defined).
class TestBase
  # Zero-arg initializer; the shim does `__t = XTest.new` per test
  # method (no Minitest-style name argument needed).
  def initialize
  end

  # Per-test isolation: shim calls `__t.setup` between `__t = .new`
  # and `__t.test_X`; we run the DB reset first so user `setup`
  # methods see fresh state. (Subclasses that override `setup`
  # invoke `super` — same Minitest before_setup → setup ordering.)
  def setup
    SchemaSetup.reset! if defined?(SchemaSetup)
  end

  # Default no-op so the shim's `__t.teardown` resolves on test
  # classes that don't define one.
  def teardown
  end

  # `assert_match` left as a method — nilable value handling differs
  # per target. spinel-target will need adjusting when toolchain-spinel
  # re-enables; for now this works under CRuby.
  def assert_match(pattern, value, msg = nil)
    raise(msg || "assert_match: expected non-nil") if value.nil?
    return if value =~ pattern
    raise(msg || "assert_match failed: expected #{value.inspect} to match #{pattern.inspect}")
  end
end

# `ActionDispatch::IntegrationTest` parent — Rails controller tests
# inherit from this. Define it as a TestBase subclass that mixes in
# RequestDispatch so the emitted `class XControllerTest <
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
  class IntegrationTest < TestBase
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
    # Controllers load on demand (the CRuby target's routes.rb no longer
    # eager-requires them; they're lazy-loaded at dispatch). The blog's
    # RequestDispatch case-table is hardcoded to articles/comments, so
    # require exactly those — idempotent on targets whose routes.rb still
    # requires controllers eagerly.
    require_relative "../app/controllers/articles_controller"
    require_relative "../app/controllers/comments_controller"
    ViewHelpers.reset_slots!
    matched = Router.match(method, path, RouteTable.table)
    raise "No route matches #{method} #{path}" if matched.nil?
    controller = case matched.controller
                 when :articles then ArticlesController.new
                 when :comments then CommentsController.new
                 end
    # Test fixtures pass Symbol-keyed nested hashes (`{article: {title:
    # ...}}`); the wire-level request body is String-keyed at runtime.
    # Stringify recursively so the harness shape matches what the
    # request-body parser would produce in production. The is_a?(Hash)
    # check is inline at the call site (not inside stringify_keys) so
    # the helper itself stays strictly typed as `(Hash) -> Hash`.
    #
    # `stringify_keys(matched.path_params)` (rather than `path_params
    # .dup`) seeds `merged` as `Hash[String, untyped]` — needed so the
    # nested-Hash branch of the ternary below has a slot wide enough
    # to hold a Hash value. `path_params.dup` keeps the StrStrHash
    # shape, which spinel then refuses to assign a Hash into.
    merged = stringify_keys(matched.path_params)
    params.each do |k, v|
      if v.is_a?(Hash)
        merged[k.to_s] = stringify_keys(v)
      else
        merged[k.to_s] = v
      end
    end
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
  #
  # Strictly typed `(Hash) -> Hash` — the polymorphism (Hash vs leaf)
  # lives at the call site's ternary, not on this function's boundary.
  # Keeps inference clean across every target's strict typer (avoids
  # the spinel #585 early-return-vs-Hash-build unification gap, and
  # the Rust/Crystal/Kotlin equivalent of "force the whole signature
  # to Value-everywhere").
  def stringify_keys(hash)
    out = {}
    hash.each do |k, v|
      if v.is_a?(Hash)
        out[k.to_s] = stringify_keys(v)
      else
        out[k.to_s] = v
      end
    end
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
    # Direct `raise unless` rather than delegating to `assert` — spinel
    # doesn't ship `Minitest::Assertions`, so the inherited `assert`
    # body emits as a vacuous 0 and lets failures pass silently. Same
    # rationale for the other helpers in this file. See
    # project_spinel_assertions_vacuous.md.
    raise "expected response #{expected.inspect}, got status=#{actual} body=#{response.body[0, 200].inspect}" unless matches
  end

  # Two-argument form retained for hand-written spinel-blog tests
  # (`assert_redirected_to "/articles/1", res`); single-argument form
  # used by emitted tests pulls from the dispatch-stashed response.
  def assert_redirected_to(expected_path, response = @__response)
    raise "expected a redirect, got status=#{response.status} location=#{response.location.inspect}" unless response.redirect?
    raise "expected redirect to #{expected_path.inspect}, got #{response.location.inspect}" unless expected_path == response.location
  end

  # `assert_select` over the Dom primitive surface (defined above). The
  # stub Dom is a substring matcher, so this is NOT yet a real CSS
  # engine — but the call shape is the real one: select nodes, assert
  # the set is non-empty, and (for the content form) assert a matched
  # node's text contains the expected string. Forms exercised by real-
  # blog: `assert_select("h1", "Articles")`, `assert_select("form")`,
  # `assert_select("#comments .p-4", minimum: 1)`, and the block form
  # `assert_select("#articles") { … }`.
  #
  # `minimum:`/`maximum:`/`count:` opts degrade to a presence check
  # (the pre-contract behavior; real-blog only passes `minimum: 1`,
  # for which presence is exact). The block runs against the same body
  # — no real scoping until a real engine lands. `opts` is retained in
  # the signature for call-shape compatibility.
  def assert_select(selector, content_or_opts = nil, opts = nil, &block)
    body  = @__response.body.to_s
    nodes = Dom.select(Dom.parse(body), selector)
    raise "expected #{selector.inspect} in response body" if nodes.empty?
    content = content_or_opts.is_a?(Hash) ? nil : content_or_opts
    if content.is_a?(String)
      needle  = content
      matched = nodes.any? { |n| Dom.text(n).include?(needle) }
      raise "expected #{selector.inspect} containing #{content.inspect} in response body" unless matched
    end
    yield if block
  end
end
