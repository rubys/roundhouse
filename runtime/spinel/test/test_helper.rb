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
require_relative "../runtime/json_impl"
require_relative "../runtime/db"
# Params — narrowing accessors the synthesized `<Resource>Params.
# from_raw` calls. THIRD entry point that needs it, alongside the two
# scaffold main.rb files: this harness builds its own require chain
# rather than loading main.rb, so a POST test hits `uninitialized
# constant Params` without it (which is what caught it — `compare`
# is GET-only and stayed green).
require_relative "../runtime/params"
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
    attrs = selector_attrs(selector.split(" ")[0].to_s)
    nodes = []
    from = 0
    while (i = root.index(fragment, from))
      # An `[attr=value]` predicate has to hold on the SAME start tag
      # the fragment matched, not merely somewhere in the document, so
      # the scan stops at the next ">". Without the predicates this is
      # the plain substring test it always was.
      stop = root.index(">", i)
      tag_end = stop.nil? ? root.length : stop
      tag = root[i, tag_end - i + 1].to_s
      ok = true
      attrs.each { |a| ok = false unless tag.include?(a) }
      nodes << root if ok
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
  # Compound selectors take the first whitespace chunk; any
  # `[attr=value]` predicates are stripped here and checked by `select`.
  def self.fragment_for(selector)
    chunk = selector.split(" ").first || ""
    first = chunk.split("[")[0].to_s
    if first.start_with?("#")
      %(id="#{first[1..]}")
    elsif first.start_with?(".")
      %(#{first[1..]}")
    else
      "<#{first}"
    end
  end

  # `turbo-stream[action='append'][target='x']` → ['action="append"',
  # 'target="x"'], each rendered the way an emitted start tag writes it
  # so the scan is one `include?` per predicate. Both quote styles are
  # accepted going in — a Rails test writes either — and normalized to
  # the double quotes every emitter produces. A bare `[connected]`
  # predicate keeps just the name.
  #
  # This engine is a substring stub, and a stub that silently answered
  # "no match" for every attribute selector made the assertion
  # UNPASSABLE rather than loose: `assert_select
  # "turbo-stream[action='append']"` could not succeed against a body
  # that contained exactly that element.
  def self.selector_attrs(chunk)
    out = []
    parts = chunk.split("[")
    i = 1
    while i < parts.length
      pred = parts[i].to_s.split("]")[0].to_s
      eq = pred.index("=")
      if eq.nil?
        out << pred
      else
        name = pred[0, eq].to_s
        value = pred[eq + 1, pred.length].to_s.gsub("'", "").gsub("\"", "")
        out << %(#{name}="#{value}")
      end
      i += 1
    end
    out
  end
end

# ---- ActiveJob::TestHelper ------------------------------------------
#
# `assert_enqueued_jobs 1, only: [ Room::PushMessageJob ] do … end` —
# run the block, count what it enqueued.
#
# THE SUITE RUNS THE `:test` ADAPTER, as Rails' does: `perform_later`
# records and returns WITHOUT dispatching (see `ActiveJob::ENQUEUE_ONLY`
# — switched on at the bottom of this file). The app itself still runs
# its jobs inline; only the suite enqueues. That is not a convenience:
# campfire's `Message` fires `Room::PushMessageJob.perform_later` from
# an `after_create_commit`, so under inline dispatch every message a
# FIXTURE loads would run the pusher, and the suite would die in its
# unresolvable nested join before a single assertion ran. Rails' suite
# never reaches that code for exactly this reason.
#
# `ActiveJob::PERFORMED` is the queue-inspection seam, appended by the
# `perform_later` wrapper `lower::job_class_side` synthesizes — a log of
# NAMES rather than a queue of arguments, which is what
# `assert_enqueued_with` documents its narrower check against.
#
# `only:` is an ARRAY OF CLASS NAMES, empty for "any". The test source
# writes classes (`only: Bot::WebhookJob`); `lower::job_test_only`
# rewrites them, because a class is not a first-class value on the
# strict targets.
#
# Length-delta rather than a clear, same as
# `capture_turbo_stream_broadcasts`: a test may assert twice in one
# method, and clearing would hide what an earlier block did.
module ActiveJob
  module TestHelper
    def capture_enqueued_jobs(only, &block)
      before = ActiveJob.performed.length
      block.call
      ActiveJob.performed[before..].select { |name| only.empty? || only.include?(name) }
    end

    def assert_enqueued_jobs(count, only: [], &block)
      actual = capture_enqueued_jobs(only, &block).length
      return if actual == count
      raise("assert_enqueued_jobs failed: expected #{count} " \
            "job(s)#{only.empty? ? "" : " of #{only.join(", ")}"}, got #{actual}")
    end

    def assert_no_enqueued_jobs(only: [], &block)
      assert_enqueued_jobs(0, only: only, &block)
    end

    # The suite runs under the `:test` adapter (see the block comment
    # above), so a job enqueued outside one of these blocks has NOT
    # run. Rails drains its queue here; we hold no arguments to replay,
    # so we switch back to inline dispatch for the block instead — the
    # jobs it enqueues run as it enqueues them, which is the same
    # observable behaviour for a block that enqueues and then asserts.
    #
    # `ensure`, and a STACK in `ActiveJob`, so a nested block and a
    # raising one both restore what they found.
    def perform_enqueued_jobs(only: [], &block)
      ActiveJob.run_enqueued
      begin
        block.call
      ensure
        ActiveJob.enqueue_without_running
      end
    end

    # `assert_enqueued_with(job: RemoveBannedContentJob, args: [ user ])`.
    #
    # DIVERGENCE, stated rather than hidden: only the JOB is checked.
    # Rails also matches the arguments, and matching them here would
    # need the log to carry them — where a record argument compares by
    # object identity, not by Rails' GlobalID, so the test's fixture and
    # the controller's freshly-loaded row would never be equal. A check
    # that fails for the wrong reason is worse than a narrower one that
    # says so. Recorded in docs/pipeline/runtime.md.
    def assert_enqueued_with(job: "", args: nil, &block)
      actual = capture_enqueued_jobs(job.empty? ? [] : [job], &block).length
      return if actual > 0
      raise("assert_enqueued_with failed: no #{job} was enqueued")
    end
  end
end

# The switch itself, at load: from here on `perform_later` enqueues and
# returns. Placed at the top level rather than in `TestBase#setup`
# because a FIXTURE load runs model callbacks before any test's setup —
# which is precisely the path (`after_create_commit` →
# `Room::PushMessageJob.perform_later`) that made this necessary.
ActiveJob.enqueue_without_running

# ---- ActionCable::TestHelper ----------------------------------------
#
# `assert_broadcasts stream, 1 do … end`. Reads `Broadcasts::LOG` — the
# same log `assert_turbo_stream_broadcasts` reads, one layer lower:
# that helper names a stream by its streamables, this one takes the
# stream string an app-defined channel composed
# (`UnreadRoomsChannel.stream_name_for(id)`).
module ActionCable
  module TestHelper
    def capture_broadcasts_on(stream, &block)
      before = Broadcasts.log.length
      block.call
      Broadcasts.log[before..].select { |entry| entry[:stream] == stream }
    end

    def assert_broadcasts(stream, count, &block)
      actual = capture_broadcasts_on(stream, &block).length
      return if actual == count
      raise("assert_broadcasts failed: expected #{count} broadcast(s) " \
            "to #{stream.inspect}, got #{actual}")
    end

    def assert_no_broadcasts(stream, &block)
      assert_broadcasts(stream, 0, &block)
    end
  end
end

# In-process request dispatch — equivalent of Rails's
# ActionDispatch::IntegrationTest. Test classes that need to exercise
# controller actions extend this module to get get/post/patch/delete.
class ActionResponse
  # `cookies` is THIS response's writes — the set the dispatcher would
  # serialize as Set-Cookie — not the browser's whole jar. Same split
  # Rails draws, and the same one `CookieJar#pending` vs `#to_h` draws.
  # Handed back as a jar rather than the raw Hash so a Symbol subscript
  # (`response.cookies[:last_room]`, which is what tests write) hits the
  # same key normalization every other read goes through.
  attr_reader :status, :body, :location, :flash, :cookies

  def initialize(status:, body:, location:, flash:, cookies:)
    @status   = status
    @body     = body
    @location = location
    @flash    = flash
    @cookies  = cookies
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
  # Rails puts both of these on `ActiveSupport::TestCase` itself, so a
  # test that never writes `include ActiveJob::TestHelper` still has
  # `perform_enqueued_jobs` — campfire's `User::BotTest` is exactly
  # that. A test that DOES write the include still resolves; including
  # a module twice is inert.
  include ActiveJob::TestHelper
  include ActionCable::TestHelper

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
    ActiveSupport.travel(0) if defined?(ActiveSupport)
  end

  # Default no-op so the shim's `__t.teardown` resolves on test
  # classes that don't define one.
  def teardown
  end

  # `dom_id(record)` — `ActionView::RecordIdentifier`, which Rails mixes
  # into integration tests so an assertion can name the element the view
  # rendered (`assert_select "#" + dom_id(@message)`). Delegates to the
  # same module function the VIEWS call, so the two cannot disagree
  # about what a record's element id is — the point of the seam.
  def dom_id(record, suffix = nil)
    ActionView::ViewHelpers.dom_id(record, suffix)
  end

  # ---- ActiveSupport::Testing::TimeHelpers -----------------------
  #
  # Rails stubs `Time.now` itself; the shared runtime cannot reopen a
  # built-in and spinel AOT cannot stub at all, so the runtime routes
  # every time read through `ActiveSupport.now` and this moves THAT.
  # See the TEST CLOCK block in
  # `runtime/spinel/active_support_time_parsing.rb`.
  #
  # WHOLE SECONDS, like Rails' own (`travel` truncates to the second);
  # the offset is relative to the real clock, so the app keeps ticking
  # from the target rather than freezing on it — which is `travel_to`'s
  # documented behaviour without a block.
  #
  # `setup` above resets it, so a travelling test cannot leak its clock
  # into the next one. That is Rails' `after_teardown travel_back`,
  # moved to setup because the shim calls `teardown` on the test's own
  # class, which may not call super.
  def travel_to(target)
    ActiveSupport.travel(target.to_i - Time.now.to_i)
    return unless block_given?
    begin
      yield
    ensure
      travel_back
    end
  end

  def travel(duration)
    travel_to(Time.now + duration.to_i)
  end

  def travel_back
    ActiveSupport.travel(0)
  end

  # `assert_match` left as a method — nilable value handling differs
  # per target. spinel-target will need adjusting when toolchain-spinel
  # re-enables; for now this works under CRuby.
  def assert_match(pattern, value, msg = nil)
    raise(msg || "assert_match: expected non-nil") if value.nil?
    return if value =~ pattern
    raise(msg || "assert_match failed: expected #{value.inspect} to match #{pattern.inspect}")
  end

  # ---- ActiveSupport::Testing::Assertions ------------------------
  #
  # Methods rather than emit-time rewrites, because each one has to
  # evaluate its expression TWICE around the block — a shape the
  # assertion rewriter (which turns `assert_equal a, b` into a raising
  # `if`) has no form for. They live on TestBase, not RequestDispatch,
  # because model tests use them too (campfire's user_test counts
  # memberships, webhook_test counts messages).
  #
  # Only the LAMBDA form is supported. Rails also takes a String to
  # `eval`, an Array of expressions, and a Hash of expression =>
  # delta; the whole corpus writes `-> { … }`, and `eval` has no
  # meaning on a compiled target.

  # `assert_difference -> { Model.count }, +1 do … end`
  def assert_difference(expression, difference = 1, message = nil, &block)
    before = expression.call
    block.call
    actual = expression.call - before
    return if actual == difference
    raise(message || "assert_difference failed: expected #{difference}, got #{actual}")
  end

  def assert_no_difference(expression, message = nil, &block)
    assert_difference(expression, 0, message, &block)
  end

  # ---- Turbo::Broadcastable::TestHelper ---------------------------
  #
  # `assert_turbo_stream_broadcasts [ users(:david), :rooms ], count: 1
  # do … end` — run the block, count what it broadcast to that stream.
  #
  # Reads `Broadcasts::LOG`, which already carries action/stream/target/
  # html for every broadcast the app makes; nothing is captured or
  # stubbed. Rails' own helper reads the ActionCable test adapter's
  # pubsub queue, which is the same fact one layer down.
  #
  # THE STREAM NAME IS THE CONTRACT. `lower::broadcasts::stream_name`
  # spells the publish side and `turbo_stream_from` the subscribe side;
  # a test that spelled it a third way would pass while the app talked
  # to nobody. `dom_id` is what both use for a record, so this asks the
  # same module function the views and the lowering ask.
  def turbo_stream_name(streamables)
    parts = streamables.is_a?(Array) ? streamables : [streamables]
    parts.map { |part|
      part.is_a?(Symbol) || part.is_a?(String) ? part.to_s : dom_id(part)
    }.join(":")
  end

  # The broadcasts a block adds to the log, filtered to one stream.
  # Length-delta rather than a clear: a test may assert twice in one
  # method, and clearing would hide what an earlier block did.
  def capture_turbo_stream_broadcasts(streamables, &block)
    before = Broadcasts.log.length
    block.call
    stream = turbo_stream_name(streamables)
    Broadcasts.log[before..].select { |entry| entry[:stream] == stream }
  end

  def assert_turbo_stream_broadcasts(streamables, count: 1, &block)
    actual = capture_turbo_stream_broadcasts(streamables, &block).length
    return if actual == count
    raise("assert_turbo_stream_broadcasts failed: expected #{count} " \
          "broadcast(s) to #{turbo_stream_name(streamables).inspect}, got #{actual}")
  end

  def assert_no_turbo_stream_broadcasts(streamables, &block)
    assert_turbo_stream_broadcasts(streamables, count: 0, &block)
  end

  # `assert_changes -> { record.reload.token } do … end`, optionally
  # pinned with `from:`/`to:`.
  #
  # DIVERGENCE: Rails distinguishes "not passed" from "passed as nil"
  # with an UNTRACKED sentinel, so `assert_changes …, to: nil` asserts
  # the value BECOMES nil. Here nil means "unspecified" — asserting a
  # change *to* nil is spelled by letting the plain change check carry
  # it. Nothing in the corpus passes either as nil.
  def assert_changes(expression, message = nil, from: nil, to: nil, &block)
    before = expression.call
    if !from.nil? && before != from
      raise(message || "assert_changes failed: expected initial #{from.inspect}, got #{before.inspect}")
    end
    block.call
    after = expression.call
    if after == before
      raise(message || "assert_changes failed: #{before.inspect} did not change")
    end
    return if to.nil? || after == to
    raise(message || "assert_changes failed: expected #{to.inspect}, got #{after.inspect}")
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
  # NO `include ActionView` / `include ActionDispatch` here, though it
  # would shorten the two references below. Including a namespace makes
  # every class nested in it resolve as a BARE constant inside every
  # test class that mixes this module in — so campfire's own
  # `Session < ApplicationRecord` lost `Session.create!` to
  # `ActionDispatch::Session`, which answers no such method. Ruby
  # searches an included module's namespace before top level, so the
  # app's own class cannot win. `Flash`, `Request` and `Cookies` are
  # the same hazard waiting for the app that names one. Two qualified
  # references are cheaper than that.

  # `headers:` carries RAW CGI env keys, the way Rails' integration
  # tests do — `"REMOTE_ADDR" => "203.0.113.1"`, `"HTTP_USER_AGENT" =>
  # "Mozilla/5.0"`. Rails also accepts wire-shaped names ("Content-Type")
  # and normalizes them; campfire only ever writes the env shape, and
  # normalizing would need a rule table nothing in the corpus exercises,
  # so the keys go through verbatim onto the env the dispatch builds.
  # Any test asserting on a request-derived value the app reads —
  # `reject_banned_ip`'s `request.remote_ip`, a push subscription's
  # `request.user_agent` — needs this to reach the env, not just the
  # params.
  #
  # `env:` is Rails' OTHER spelling of the same thing and campfire
  # writes both (`get new_session_url, env: { "HTTP_USER_AGENT" => … }`
  # in `sessions_controller_test`). In Rails the two differ only in
  # normalization — `headers:` accepts wire-shaped names and rewrites
  # them, `env:` is raw — and since this harness takes `headers:` RAW
  # already, they are the same hash here. Merged with `env:` last, so a
  # test naming a key in both gets the rawer one, which is Rails'
  # order too.
  def get(path, params: {}, headers: {}, env: {})
    dispatch_request("GET", path, params, headers.merge(env))
  end

  def post(path, params: {}, headers: {}, env: {})
    dispatch_request("POST", path, params, headers.merge(env))
  end

  def patch(path, params: {}, headers: {}, env: {})
    dispatch_request("PATCH", path, params, headers.merge(env))
  end

  # Rails' integration tests define all five verbs plus `head`; the four
  # the blog happened to use were the four that existed. campfire's
  # `put account_user_url(...)` is the first `put` in the corpus, and it
  # failed as a missing METHOD rather than as an unrouted request.
  def put(path, params: {}, headers: {}, env: {})
    dispatch_request("PUT", path, params, headers.merge(env))
  end

  def delete(path, params: {}, headers: {}, env: {})
    dispatch_request("DELETE", path, params, headers.merge(env))
  end

  def head(path, params: {}, headers: {}, env: {})
    dispatch_request("HEAD", path, params, headers.merge(env))
  end

  # Rails' `follow_redirect!` — re-issue the LAST response's redirect as
  # a GET. campfire's `rooms/closeds_controller_test` revises a room's
  # membership so as to remove itself, then follows the redirect to the
  # room to check it bounces on to root.
  #
  # GET only, and it raises when the last response was not a redirect —
  # both are Rails' own contract, not a subset: `follow_redirect!` is
  # defined as "the browser follows a 3xx", and a browser follows one
  # with a GET.
  def follow_redirect!
    unless @__response.redirect?
      raise "follow_redirect! called at a non-redirect response (status=#{@__response.status})"
    end
    get(@__response.location)
  end

  # The browser. An integration test's requests share cookie state the
  # way a real client does — `sign_in` POSTs, and every later request in
  # that test carries the session cookie — and a test may seed one before
  # the first request (campfire's `cookies[:last_room] = room.id`).
  # Per-test lifetime, like `@__session` / `@__flash` beside it: the shim
  # builds a fresh test instance per test method, so a fresh jar comes
  # with it.
  def cookies
    @__cookies = ActionController::CookieJar.new if @__cookies.nil?
    @__cookies
  end

  # The request the last dispatch built, as Rails' integration tests
  # expose it. campfire rebuilds a jar around it to read back a signed
  # cookie (`ActionDispatch::Cookies::CookieJar.build(request, …)`).
  def request
    @__request
  end

  # Its twin, and missing for as long as `request` has been here.
  # `@response` was assigned (the ivar spelling a test writes directly)
  # but the bare READER was not, so `response.cookies[:last_room]` and
  # `response.parsed_body` — the spelling Rails' integration tests
  # actually use — were a NoMethodError on the test instance.
  def response
    @__response
  end

  # `host! "once.campfire.test"` — the Host every subsequent request in
  # this test carries. Rails' integration tests set it when the app
  # reads the host back: campfire's messages_controller_test does,
  # because a message's attachment URLs are absolute. Per-test, like the
  # cookie jar; the default is the one the env below always used.
  def host!(name)
    @__host = name
  end

  def host
    @__host = "example.org" if @__host.nil?
    @__host
  end

  def dispatch_request(method, path, params, headers = {})
    require_relative "../config/routes"
    # Controllers load on demand (the CRuby target's routes.rb no longer
    # eager-requires them; they're lazy-loaded at dispatch). The blog's
    # RequestDispatch case-table is hardcoded to articles/comments, so
    # require exactly those — idempotent on targets whose routes.rb still
    # requires controllers eagerly.
    require_relative "../app/controllers/articles_controller"
    require_relative "../app/controllers/comments_controller"
    ActionView::ViewHelpers.reset_slots!
    # `[RouteTable.root] + RouteTable.table`, exactly as both production
    # dispatchers compose it: `root "c#a"` is kept as its own constant
    # (it is the only literal-pattern entry) and the table walk stays
    # flat. The harness searched `table` alone, so `get "/"` — the one
    # request every app answers — was "No route matches" in a controller
    # test and a 200 in production.
    # SPLIT THE QUERY OFF BEFORE MATCHING. A route pattern describes a
    # PATH; `?before=6` is not part of one, and the router compared it
    # against the pattern verbatim and answered "No route matches GET
    # /rooms/3/messages?before=6". Every production dispatcher splits
    # here (main.rb reads PATH_INFO and QUERY_STRING as separate env
    # keys), and the env built further down already partitioned the
    # same string — the match was simply reading the unsplit one.
    #
    # campfire's `messages_controller_test` pages with
    # `room_messages_url(@room, before: @messages.third)`, which is how
    # a query string reaches a test path at all: a route helper renders
    # its non-segment options into one.
    match_path, _, query = path.partition("?")
    matched = ActionDispatch::Router.match(
      method, match_path, [RouteTable.root] + RouteTable.table
    )
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
    # `post url, params: "Hello Bot World!"` — Rails' integration tests
    # take a STRING `params:` as the raw request BODY, not as a
    # parameter hash. campfire's bot endpoint is written that way (a bot
    # POSTs plain text and the controller reads `request.body`), and the
    # harness iterated it: `undefined method 'each' for an instance of
    # String`, every test in the file.
    # Query-string pairs are parameters too, and they arrive UNDER the
    # explicit `params:` — Rails merges the request's own parameters
    # first and lets the caller's hash win on a collision.
    # `CgiIo.parse_form_into` — THE SAME PARSER the production
    # dispatcher uses on `QUERY_STRING` (`parse_request` calls it on
    # exactly this string), so the harness cannot disagree with
    # production about what a query string means. Not `CGI.parse`: Ruby
    # 4.0 no longer ships it, and a hand-rolled split would be a second
    # copy of a table this tree already owns.
    CgiIo.parse_form_into(query, merged) unless query.empty?
    request_body = +""
    if params.is_a?(String)
      request_body = params.to_s
    else
      params.each do |k, v|
        if v.is_a?(Hash)
          merged[k.to_s] = stringify_keys(v)
        else
          merged[k.to_s] = v
        end
      end
    end
    controller.params  = merged
    controller.session = @__session ||= ActionDispatch::Session.new
    controller.flash   = @__flash   ||= ActionDispatch::Flash.new
    # The inbound jar is the browser's whole state — what the test has
    # accumulated from earlier responses PLUS anything it seeded itself,
    # which is exactly `to_h`. A fresh jar per request, so `pending`
    # holds only this response's writes, matching what both production
    # dispatchers drain (main.rb's `controller.cookies.pending` loop).
    controller.cookies = ActionController::CookieJar.new(cookies.to_h)
    controller.request_method = method
    controller.request_path   = path
    # The response FORMAT, derived exactly as both production
    # dispatchers derive it: a `(.:format)` extension the router
    # stripped off the path, then a route-pinned format on top. The
    # harness derived neither, so `get room_refresh_url(room, format:
    # :turbo_stream)` — a URL whose whole point is the format — arrived
    # as :html and the action fell through to MissingTemplate.
    #
    # Compared against string literals rather than converted with
    # `to_sym`, the same shape main.rb uses and for the same reason: a
    # Symbol materialized from a runtime String is not a shape the
    # strict targets share.
    path_format = matched.path_params.fetch("format", "")
    controller.request_format = :json if path_format == "json"
    controller.request_format = :turbo_stream if path_format == "turbo_stream"
    controller.request_format = :rss if path_format == "rss"
    controller.request_format = :json if matched.req_format == :json
    controller.request_format = :rss if matched.req_format == :rss
    # The request object, built the way the dispatcher builds it (see
    # `main.rb`'s `controller.request = ActionDispatch::Request.new(...)`).
    # Without it `controller.request` was nil and any filter touching it
    # died before the action ran — campfire's `reject_banned_ip` reads
    # `request.remote_ip` on EVERY request, so all 29 of its controller
    # tests stopped there. The blog's controller tests never touch the
    # request, which is why the harness got this far without one.
    #
    # `Request.for` rather than `.new`: the shared class and the CRuby
    # overlay's twin hold their state differently and take different
    # constructors. Values are the loopback defaults a test wants —
    # a test that needs a specific one passes `headers:`, which is
    # applied OVER the defaults below (so a test can override
    # REMOTE_ADDR / HTTP_USER_AGENT) but under the four keys the
    # dispatch itself owns.
    request_path, _, request_query = path.partition("?")
    env = {
      "HTTP_HOST"       => host,
      "REMOTE_ADDR"     => "127.0.0.1",
      "HTTP_USER_AGENT" => "Roundhouse Test",
    }
    headers.each { |k, v| env[k.to_s] = v.to_s }
    env["REQUEST_METHOD"] = method
    env["PATH_INFO"]      = request_path
    env["QUERY_STRING"]   = request_query
    controller.request = ActionDispatch::Request.for(env, merged)
    controller.request.body = request_body
    # Same object where module-function helpers reach it, and the
    # controller alongside — mirrors the dispatcher's pair.
    ActionController::Current.request = controller.request
    ActionController::Current.controller = controller
    @__request = controller.request
    controller.process_action(matched.action)
    @__flash = controller.flash
    # Fold this response's Set-Cookie writes back into the browser.
    @__cookies = ActionController::CookieJar.new(
      accept_cookies(cookies.to_h, controller.cookies.pending)
    )
    @__response = ActionResponse.new(
      status:   controller.status,
      body:     controller.body,
      location: controller.location,
      flash:    controller.flash,
      cookies:  ActionController::CookieJar.new(controller.cookies.pending),
    )
    # Rails' OWN names, alongside the `__`-prefixed ones the harness
    # methods read. An integration test writes `@response.body` and
    # `@response.content_type` directly — campfire's
    # `users/sidebars_controller_test` asserts on the rendered body that
    # way, and got `undefined method 'body' for nil`. The prefixed pair
    # stays because a test class may define its own `@response`; these
    # two are what the test SOURCE says, so they are assigned last and a
    # test that overwrites one only affects itself.
    @request = @__request
    @response = @__response
    @__response
  end

  # What a browser does with a response's Set-Cookie lines: take the
  # writes over what it was already holding, and DROP the ones that came
  # back cleared. `CookieJar#delete` records a cleared write as "" (the
  # jar has no tombstone type — see runtime/ruby/action_controller/
  # cookies.rb), and both production dispatchers turn that into an
  # expiring Set-Cookie. Keeping the key here instead would leave
  # `cookies[:session_token].present?` true after a sign-out, which is
  # precisely the assertion campfire's sessions_controller_test makes.
  #
  # `merge` then delete, rather than accumulating into a `{}` literal:
  # a bare-Hash seed types its values Untyped, and both operands here
  # are already Hash[String, String].
  def accept_cookies(carried, pending)
    merged = carried.merge(pending)
    pending.each { |k, v| merged.delete(k) if v == "" }
    merged
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
  # ASSERTION-ONLY aliases: Rails lets a controller test say
  # `assert_response :success` and match any 2xx. These are ranges, and
  # they are not statuses a response can be SET to — which is why they
  # do not belong in `ActionController::STATUS_CODES` and why every
  # real status here is read from that table instead of restated.
  #
  # This file used to carry its own copy of the registry, and campfire
  # found what two copies cost: `:too_many_requests` was missing from
  # BOTH, so `head :too_many_requests` answered 200 and then the
  # assertion that should have caught it could not name a 429 either.
  # Ranges only, so the Hash stays monomorphic on its value type.
  STATUS_RANGES = {
    success:  200..299,
    redirect: 300..399,
    missing:  404..404,
    error:    500..599,
  }.freeze

  def assert_response(expected, response = @__response)
    actual = response.status
    matches = if expected.is_a?(Symbol)
                range = STATUS_RANGES[expected]
                if range.nil?
                  code = ActionController::STATUS_CODES[expected]
                  raise "assert_response: unknown status #{expected.inspect}" if code.nil?
                  code == actual
                else
                  range.include?(actual)
                end
              else
                expected == actual
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
