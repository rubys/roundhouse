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
  # Stub: one synthetic node (the root's html) per matching element, so
  # nested selects re-scan the whole string (the historical no-scoping
  # block behavior).
  #
  # The scan is anchor-then-verify. `fragment_for` picks ONE part of the
  # compound selector as a cheap way to enumerate candidate positions;
  # every part is then checked against the START TAG the candidate sits
  # in, which is recovered by walking back to its `<`. That walk is what
  # lets a CLASS anchor work at all: a class name is matched mid-tag, so
  # scanning forward from the hit gives a suffix of the tag rather than
  # the tag.
  def self.select(root, selector)
    chunk = target_chunk(selector)
    base = chunk.split("[")[0].to_s
    want_tag = selector_tag(base)
    want_id = selector_id(base)
    want_classes = selector_classes(base)
    attrs = selector_attrs(chunk)
    fragment = fragment_for(selector)
    nodes = []
    return nodes if fragment == ""
    from = 0
    while (i = root.index(fragment, from))
      from = i + fragment.length
      start = tag_start(root, i)
      stop = root.index(">", i)
      tag_end = stop.nil? ? root.length : stop
      tag = root[start, tag_end - start + 1].to_s
      ok = tag_named?(tag, want_tag)
      ok = false if want_id != "" && !tag.include?(%(id="#{want_id}"))
      if ok && want_classes.length > 0
        present = tag_classes(tag)
        want_classes.each { |c| ok = false unless present.include?(c) }
      end
      attrs.each { |a| ok = false unless tag.include?(a) }
      nodes << root if ok
    end
    nodes
  end

  # Concatenated descendant text of a node. Stub: the node's html
  # verbatim (so a content check degrades to a body-substring check).
  def self.text(node)
    node
  end

  # The index of the `<` that opens the tag position `i` sits inside.
  def self.tag_start(root, i)
    j = i
    while j >= 0
      return j if root[j, 1] == "<"
      j -= 1
    end
    0
  end

  # Does this start tag name the element the selector asked for? An
  # empty want matches anything (the selector named no tag). The
  # boundary check is why this is not a bare `start_with?`: `<hr` is a
  # prefix of `<hrefish` too.
  def self.tag_named?(tag, want)
    return true if want == ""
    return false unless tag.start_with?("<" + want)
    after = tag[want.length + 1, 1].to_s
    after == "" || after == " " || after == ">" || after == "/" || after == "\n"
  end

  # The tokens of this start tag's `class` attribute. WHOLE tokens, not
  # a substring: `.message` must not hold on `class="message__body"`,
  # and the historical rule (the class attribute ENDS with the name)
  # could not say that — nor could it match a class that was not last
  # (`assert_select "#comments .p-4"` against `class="p-4 bg-gray-50
  # rounded"`, which is real-blog's comment partial).
  def self.tag_classes(tag)
    at = tag.index("class=\"")
    return [] if at.nil?
    rest = tag[at + 7, tag.length].to_s
    close = rest.index("\"")
    value = close.nil? ? rest : rest[0, close].to_s
    value.split(" ")
  end

  # The chunk the assertion is ABOUT — the LAST one, not the first.
  #
  # `assert_select "turbo-frame#account_users hr.separator.full-width"`
  # asserts an `hr` INSIDE that frame. This engine cannot scope, so the
  # honest degradation is to check the TARGET and ignore the ancestor;
  # taking the first chunk instead checked the ancestor and said nothing
  # about the thing the assertion names. Worse than loose, it was
  # UNPASSABLE: a compound first chunk (`turbo-frame#account_users`)
  # fell through to the tag rule and asked the document to contain the
  # literal text `<turbo-frame#account_users`, which no emitter writes.
  #
  # Combinators (`>`, `+`, `~`) are chunks of their own under a
  # whitespace split; skip them so `a > b` still targets `b`.
  #
  # The split is BRACKET-AWARE, and that is not defensive coding: an
  # attribute predicate can hold a space (`.btn[title='Copy link']`, a
  # real campfire assertion), and a plain `split(" ")` cuts it in half.
  def self.target_chunk(selector)
    best = ""
    buf = ""
    depth = 0
    i = 0
    while i < selector.length
      c = selector[i, 1].to_s
      if c == "["
        depth += 1
        buf = buf + c
      elsif c == "]"
        depth -= 1 if depth > 0
        buf = buf + c
      elsif depth == 0 && (c == " " || c == "\t" || c == "\n")
        best = buf if element_chunk?(buf)
        buf = ""
      else
        buf = buf + c
      end
      i += 1
    end
    best = buf if element_chunk?(buf)
    best
  end

  # A chunk that names an element, as opposed to an empty run or a
  # combinator sitting between two of them.
  def self.element_chunk?(chunk)
    chunk != "" && chunk != ">" && chunk != "+" && chunk != "~"
  end

  # The substring `select` scans for. NOT the whole selector — just a
  # cheap enumerator of candidate positions, since `select` re-checks
  # every part against the start tag it lands in.
  #
  #   "hr.separator.full-width" → "<hr"
  #   "#account_users"          → 'id="account_users"'
  #   ".message"                → "message"
  #
  # Tag first (`<hr` is anchored to a start-tag boundary and is the
  # cheapest filter), then id, then the first class name. A bare class
  # name is the loosest of the three — it can hit inside an unrelated
  # attribute or in text — which costs candidates, not correctness:
  # a false candidate fails the class check that follows.
  def self.fragment_for(selector)
    base = target_chunk(selector).split("[")[0].to_s
    tag = selector_tag(base)
    id = selector_id(base)
    classes = selector_classes(base)
    if tag != ""
      "<" + tag
    elsif id != ""
      %(id="#{id}")
    elsif classes.length > 0
      classes[0]
    else
      ""
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

  # The three parts of one compound selector chunk. Split by hand rather
  # than by Regexp: this file is compiled by spinel AOT, where the
  # pattern surface is not the place to spend a dependency.
  def self.selector_tag(base)
    without_id(base).split(".")[0].to_s
  end

  def self.selector_id(base)
    hash = base.index("#")
    return "" if hash.nil?
    rest = base[hash + 1, base.length].to_s
    dot = rest.index(".")
    dot.nil? ? rest : rest[0, dot].to_s
  end

  def self.selector_classes(base)
    parts = without_id(base).split(".")
    out = []
    i = 1
    while i < parts.length
      out << parts[i].to_s if parts[i].to_s != ""
      i += 1
    end
    out
  end

  # `hr#x.a` → `hr.a`: the id lifted out so the remainder splits on "."
  # into tag + classes with no special case.
  def self.without_id(base)
    hash = base.index("#")
    return base if hash.nil?
    head = base[0, hash].to_s
    rest = base[hash + 1, base.length].to_s
    dot = rest.index(".")
    dot.nil? ? head : head + rest[dot, rest.length].to_s
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
#
# Counts BOTH kinds of entry the log carries — a turbo fragment
# (`Broadcasts.record`) and a raw publish (`ActionCable.server
# .broadcast`, logged with a `payload:` instead of `html:`) — which is
# what Rails' own helper counts, because its pubsub queue holds whatever
# was published. Filtering on `:stream` alone is what makes that work.
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

# `ActionDispatch::Http::UploadedFile` — what `fixture_file_upload`
# hands back, and what a multipart request would carry in production.
#
# Read-only and file-backed: nothing here writes, because the only
# producer is the test harness naming a fixture that already exists on
# disk. A real multipart parse would construct the same shape over a
# tempfile.
module ActionDispatch
  module Http
    class UploadedFile
      attr_reader :original_filename, :content_type

      def initialize(path, content_type)
        @path = path
        @original_filename = File.basename(path)
        @content_type = content_type
      end

      def read
        File.binread(@path)
      end

      def size
        File.size(@path)
      end

      # A params hash carrying one stringifies to the uploaded name,
      # which is what Rails' own `to_s` gives.
      def to_s
        @original_filename
      end
    end
  end
end

# `ActionDispatch::TestProcess` — where Rails keeps `fixture_file_upload`
# (in its `FixtureFile` submodule, which this module includes). A plain
# `ActiveSupport::TestCase` does NOT get it for free, which is why
# campfire's `Message::AttachmentTest` writes the `include` itself — so
# the module has to EXIST under that name, not just the method under
# some base class. `TestBase` includes it too, because a test that never
# writes the include still expects the method (Rails puts it on
# `ActionController::TestCase` and `ActionDispatch::IntegrationTest`).
#
# Rails hands back an `ActionDispatch::Http::UploadedFile`. That object
# above is its read surface and no more: the name it was uploaded under,
# its declared type, and its bytes.
#
# The bytes only exist here because binary passthrough carries the
# fixture files into the emitted tree; before that this method would
# have described a file that was not there.
module ActionDispatch
  module TestProcess
    def fixture_file_upload(name, content_type = "application/octet-stream")
      ActionDispatch::Http::UploadedFile.new(
        File.join(__dir__, "fixtures", "files", name), content_type
      )
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
  attr_reader :status, :body, :location, :flash, :cookies, :content_type

  def initialize(status:, body:, location:, flash:, cookies:, content_type: "text/html; charset=utf-8",
                 cache_control_max_age: 0, cache_control_public: false)
    @status   = status
    @body     = body
    @location = location
    @flash    = flash
    @cookies  = cookies
    @content_type = content_type
    @cache_control_max_age = cache_control_max_age
    @cache_control_public = cache_control_public
  end

  # Rails' `response.cache_control` — the one place the two TYPED
  # controller readers (see ActionController::Base) are re-assembled
  # into the mixed Hash Rails hands back, because the subscript
  # spelling is what a test writes:
  #
  #   assert_equal 1.year, response.cache_control[:max_age].to_i
  #   assert response.cache_control[:public]
  #
  # `:public` is ABSENT rather than false when the response is private,
  # which is Rails' own shape and what makes the bare truthiness
  # assertion above mean what it says.
  def cache_control
    out = { max_age: @cache_control_max_age }
    out[:public] = true if @cache_control_public
    out
  end

  # Rails' `response.parsed_body` — the body decoded according to the
  # response's own content type.
  #
  # JSON ONLY, which is the whole of what the corpus asks for
  # (campfire's autocomplete reads `response.parsed_body.first["name"]`).
  # Rails also hands back a Nokogiri document for html and a
  # `Rack::Utils` hash for form-encoded; both would be a parser this
  # harness does not have, and answering the raw String for them would
  # be worse than refusing — `parsed_body["k"]` on a String is a
  # TypeError three lines from where the test actually went wrong.
  #
  # `JSON` is the tree's own (stdlib under CRuby/JRuby, the bundled spin
  # package under AOT), resolved by the `runtime/json_impl` require at
  # the head of this file — which is why the harness can call it without
  # knowing which one it got.
  def parsed_body
    unless @content_type.include?("json")
      raise "parsed_body: no parser for #{@content_type.inspect} " \
            "(this harness decodes JSON only)"
    end
    JSON.parse(@body)
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
  include ActionDispatch::TestProcess

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
    # AFTER the schema reset, which reloads fixtures — and our fixture
    # loader runs model callbacks, so a broadcasting `after_create_commit`
    # would otherwise leave entries in the log before the test began.
    # Rails clears its Action Cable test adapter between tests for the
    # same reason; without this, the cumulative counting
    # `assert_turbo_stream_broadcasts` does (see its note) would carry
    # one test's broadcasts into the next.
    Broadcasts.reset_log! if defined?(Broadcasts)
    # WebMock's stub registry is a GLOBAL, and its Minitest integration
    # empties it in an `after_teardown` hook this TestBase never runs —
    # the helper is deliberately Minitest-free, which is the same reason
    # mocha is wired by hand. Without this, a `stub_request` outlives the
    # test that wrote it: campfire's Opengraph fetch tests register a 302
    # to another host early in the file, and every later test followed it
    # to a hostname its own stubs had never heard of.
    WebMock.reset! if defined?(WebMock)
  end

  # Default no-op so the shim's `__t.teardown` resolves on test
  # classes that don't define one.
  def teardown
  end

  # `file_fixture("pixel.bmp")` — Rails' handle on a file under
  # `test/fixtures/files`. A Pathname, as Rails returns, so the `.open`
  # every call site chains resolves without inventing a type.
  #
  # Anchored on `__dir__` rather than the process's cwd: the suite runs
  # each file as `ruby -Itest test/models/x_test.rb` from the emit root
  # today, and a helper that silently depends on that is a trap for the
  # first runner that does otherwise.
  #
  # These files reach the emitted tree at all only because binary
  # passthrough carries them (`App#binary_assets`) — every emitted
  # file's content is a String, so before that a `.jpg` could not be
  # represented and this method would have pointed at nothing.
  def file_fixture(name)
    Pathname.new(File.join(__dir__, "fixtures", "files", name))
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

  # Every broadcast on that stream SO FAR IN THIS TEST — cumulative,
  # not a delta, and that is turbo-rails' behavior at the version
  # campfire pins, read from the gem rather than assumed:
  #
  #   2.0.16 (campfire's pin), 2.0.17, 2.0.20   `block&.call; broadcasts(name)`
  #   2.0.21, 2.0.23                            `new_broadcasts_from(...)` — a delta
  #
  # A delta was the obvious reading and it is wrong here. campfire's
  # `rooms/involvements_controller_test` puts twice, one broadcast each,
  # and asserts `count: 1` then `count: 2` — which only counts if the
  # second block sees the first block's broadcast too. Ours took a
  # length-delta, answered 1, and the test failed against a correct app.
  # `TestBase#setup` clears the log so "this test" is the window, the
  # same bound Rails gets from clearing its test adapter.
  #
  # `assert_broadcasts` next door stays a DELTA on purpose: Action
  # Cable's own helper takes one (`new_broadcasts_from`) in every
  # version. The two helpers really do differ.
  def capture_turbo_stream_broadcasts(streamables, &block)
    block.call
    stream = turbo_stream_name(streamables)
    Broadcasts.log.select { |entry| entry[:stream] == stream }
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
      content_type: controller.content_type,
      cache_control_max_age: controller.cache_control_max_age,
      cache_control_public: controller.cache_control_public,
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
  # `minimum:`/`maximum:` and a NON-ZERO `count:` degrade to a presence
  # check (the pre-contract behavior; real-blog only passes `minimum:
  # 1`, for which presence is exact). The block runs against the same
  # body — no real scoping until a real engine lands. `opts` is retained
  # in the signature for call-shape compatibility.
  #
  # `count: 0` IS honored, and the history of that is worth keeping.
  # Honoring the whole option was tried and backed out (2026-08-22,
  # measured on campfire: 189/240 -> 184/240) because it needed TWO
  # things the stub did not have:
  #
  #   * ELEMENT counting. `Dom.select` fabricated one node per SUBSTRING
  #     occurrence, so `.message` (fragment `message"`) counted every
  #     attribute value ending in `message`. searches_controller's two
  #     `count: 0` assertions went red against a correct empty page.
  #   * BLOCK SCOPING. `assert_select "template", count: 1` inside
  #     `assert_select "turbo-stream[action='append']" do … end` asks
  #     "one template IN THIS ELEMENT". The block re-scans the whole
  #     body, which holds two.
  #
  # The FIRST of those is now fixed — `Dom.select` matches whole class
  # tokens on a real start tag — which is what makes `count: 0`
  # separable from the rest of the option: absence is a question about
  # the SELECTOR, and the selector is now answered exactly. Counting to
  # a number above zero still is not, because that is the scoping
  # problem, and scoping is the real engine's job.
  #
  # One deliberate divergence: a content constraint beside `count: 0`
  # (`assert_select "h1", text: /…/, count: 0`) is IGNORED, so the
  # assertion reads "no such element" rather than Rails' "no such
  # element WITH THIS TEXT". Narrowing it is not available here —
  # `Dom.text` answers the whole document for every node — and of the
  # two available readings, the strict one fails loudly where the loose
  # one passed vacuously.
  def assert_select(selector, content_or_opts = nil, opts = nil, &block)
    body  = @__response.body.to_s
    nodes = Dom.select(Dom.parse(body), selector)
    options = content_or_opts.is_a?(Hash) ? content_or_opts : opts
    if options.is_a?(Hash) && options[:count] == 0
      raise "expected no #{selector.inspect} in response body" unless nodes.empty?
      return
    end
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
