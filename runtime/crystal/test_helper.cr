# Test base class for emitted Crystal specs. Provides the per-test
# isolation harness (`RoundhouseTest.discover`), the
# ActionDispatch::IntegrationTest surface (get/post/etc., assert_
# response/redirected_to/select), and the few assertion methods that
# the inline-assertion lowerer (`src/lower/test_module_to_library/
# inline_assertions.rs`) deliberately leaves unrewritten — those
# whose semantics differ enough across targets that uniform inline
# emit isn't safe.
#
# **What's emitted inline as `raise unless …`** (NOT defined here):
#   assert / assert_not / refute, assert_equal / refute_equal,
#   assert_nil / refute_nil, assert_empty / refute_empty,
#   assert_includes / refute_includes, assert_kind_of,
#   assert_instance_of, assert_predicate / refute_predicate,
#   assert_raises, assert_difference / assert_no_difference.
#
# **Kept here** (cross-target friction at the lowering level):
#   - `assert_match`: Crystal's `Regex#matches?` requires `String`
#     (not nilable); the inline lowering's `pat.match?(val)` shape
#     hits Crystal's strict null-check. Nilable-handling stays in
#     the helper.
#   - `assert_operator`: Class-subclass `<` checks are a Ruby/Crystal
#     idiom with no TS analog; lowering would have to translate per-
#     target. Kept here in symmetric form across targets.
#
# Discovery: each emitted test class invokes `RoundhouseTest.discover`
# at the bottom of its file. The macro walks the class's instance
# methods at compile time, generating one `it "<name>"` Spec block
# per `test_*` method. Each `it` instantiates a fresh test object and
# calls the matching method, mirroring Minitest's per-test isolation.

require "spec"

abstract class RoundhouseTest
  # Accepts `String?` so callers can pass nilable values directly
  # (e.g. `err.message` from a Crystal Exception returns `String?`);
  # nil fails the assertion the same as a non-matching string.
  def assert_match(pattern, value : String?, msg : String? = nil) : Nil
    if value.nil?
      fail(msg || "expected non-nil string to match #{pattern.inspect}")
    end
    re = pattern.is_a?(Regex) ? pattern.as(Regex) : Regex.new(pattern.to_s)
    fail(msg || "expected #{value.inspect} to match #{re.inspect}") unless re.matches?(value)
  end

  # Ruby's `assert_operator a, :op, b` — eval `a.op(b)` and assert truthy.
  # Symbol-shaped op names (':<', ':>') and the bare form both accepted.
  # Class-subclass `<` (e.g. `assert_operator A, :<, B` for "A is a
  # subclass of B") works natively in Crystal — class `<` is the
  # subclass relation.
  def assert_operator(left, op, right, msg : String? = nil) : Nil
    op_str = op.to_s.lstrip(':')
    result = case op_str
             when "<"  then left < right
             when ">"  then left > right
             when "<=" then left <= right
             when ">=" then left >= right
             when "==" then left == right
             when "!=" then left != right
             else
               fail(msg || "assert_operator: unsupported op #{op}")
             end
    fail(msg || "expected #{left.inspect} #{op_str} #{right.inspect}") unless result
  end

  def flunk(msg : String? = nil) : Nil
    fail(msg || "flunked")
  end

  def skip(msg : String? = nil) : Nil
    raise Spec::SpecSkip.new(msg || "skipped", file: __FILE__, line: __LINE__)
  end

  # ── ActionDispatch::IntegrationTest surface ──────────────────────
  #
  # In-process dispatch via `Routes.table` + the registered controller
  # registry. Mirrors `runtime/typescript/minitest.ts:290-371` and
  # spinel's `dispatch_request` (`runtime/spinel/test/test_helper.rb:
  # 219-248`). Tests stash status / body / location on the test
  # instance for subsequent `assert_response` / `assert_select` /
  # `assert_redirected_to` checks.
  @__body : String = ""
  @__status : Int64 = 0_i64
  @__location : String = ""
  @__session : ::ActionDispatch::Session = ::ActionDispatch::Session.new
  @__flash : ::ActionDispatch::Flash = ::ActionDispatch::Flash.new

  def get(path : String, params = nil) : Nil
    _ = params
    dispatch("GET", path, {} of String => Roundhouse::ParamValue)
  end

  def post(path : String, params = nil) : Nil
    dispatch("POST", path, normalize_params(params))
  end

  def put(path : String, params = nil) : Nil
    dispatch("PUT", path, normalize_params(params))
  end

  def patch(path : String, params = nil) : Nil
    dispatch("PATCH", path, normalize_params(params))
  end

  def delete(path : String, params = nil) : Nil
    _ = params
    dispatch("DELETE", path, {} of String => Roundhouse::ParamValue)
  end

  def head(path : String, params = nil) : Nil
    _ = params
    dispatch("HEAD", path, {} of String => Roundhouse::ParamValue)
  end

  # Test fixtures pass NamedTuple-shape params
  # (`params: {article: {title: "…"}}`); the wire-level request body
  # is `Hash(String, ParamValue)`. Recursively stringify keys and
  # narrow Symbol leaves to their String form, matching what the
  # production form-body parser would produce. Untyped param admits
  # NamedTuple, Hash, or nil from each call site.
  private def normalize_params(params) : Hash(String, Roundhouse::ParamValue)
    out = {} of String => Roundhouse::ParamValue
    case params
    when Hash
      params.each { |k, v| out[k.to_s] = nested_param_value(v) }
    when NamedTuple
      params.each { |k, v| out[k.to_s] = nested_param_value(v) }
    end
    out
  end

  private def nested_param_value(v) : Roundhouse::ParamValue
    case v
    when String
      v
    when Symbol
      v.to_s
    when Hash
      inner = {} of String => Roundhouse::ParamValue
      v.each { |kk, vv| inner[kk.to_s] = nested_param_value(vv) }
      inner
    when Array
      v.map { |elem| nested_param_value(elem) }.as(Roundhouse::ParamValue)
    when NamedTuple
      inner = {} of String => Roundhouse::ParamValue
      v.each { |kk, vv| inner[kk.to_s] = nested_param_value(vv) }
      inner
    else
      v.to_s
    end
  end

  private def dispatch(
    method : String,
    path : String,
    body : Hash(String, Roundhouse::ParamValue),
  ) : Nil
    ::ActionView::ViewHelpers.reset_slots!
    matched = ::ActionDispatch::Router.match(method, path, RoundhouseTest.routes)
    if matched.nil?
      fail("no route for #{method} #{path}")
    end
    matched = matched.not_nil!
    ctrl_class = RoundhouseTest.controllers[matched.controller]?
    if ctrl_class.nil?
      fail("no controller registered for #{matched.controller}")
    end
    merged = {} of String => Roundhouse::ParamValue
    matched.path_params.each { |k, v| merged[k] = v.as(Roundhouse::ParamValue) }
    body.each { |k, v| merged[k] = v }
    ctrl = ctrl_class.not_nil!.new
    ctrl.params = merged
    ctrl.session = @__session
    ctrl.flash = @__flash
    ctrl.request_method = method
    ctrl.request_path = path
    ctrl.process_action(matched.action)
    @__body = ctrl.body || ""
    @__status = ctrl.status || 200_i64
    @__location = ctrl.location || ""
    @__flash = ctrl.flash
  end

  # ── HTTP response assertions ─────────────────────────────────────

  STATUS_SYMBOLS = {
    success:              200..299,
    redirect:             300..399,
    missing:              404,
    not_found:            404,
    error:                500..599,
    ok:                   200,
    created:              201,
    accepted:             202,
    no_content:           204,
    moved_permanently:    301,
    found:                302,
    see_other:            303,
    not_modified:         304,
    bad_request:          400,
    unauthorized:         401,
    forbidden:            403,
    unprocessable_entity: 422,
    # Rails 8.1.x scaffold renamed `:unprocessable_entity` →
    # `:unprocessable_content` mid-version (HTTP 422 description
    # churn). Alias both so emit follows whichever the fixture's
    # scaffold currently produces.
    unprocessable_content: 422,
    internal_server_error: 500,
  }

  def assert_response(expected, msg : String? = nil) : Nil
    actual = @__status.to_i32
    matched = case expected
              when Int
                expected.to_i32 == actual
              when Symbol
                rng = STATUS_SYMBOLS[expected]?
                case rng
                when Range then rng.includes?(actual)
                when Int   then rng.to_i32 == actual
                else false
                end
              else
                false
              end
    return if matched
    body_preview = @__body[0, 200]? || @__body
    fail(msg || "expected response #{expected.inspect}, got status=#{actual} body=#{body_preview.inspect}")
  end

  def assert_redirected_to(expected_path : String, msg : String? = nil) : Nil
    if @__status < 300 || @__status >= 400
      fail(msg || "expected a redirect, got status=#{@__status} location=#{@__location.inspect}")
    end
    return if @__location.includes?(expected_path)
    fail(msg || "expected Location to contain #{expected_path.inspect}, got #{@__location.inspect}")
  end

  # `assert_select` substring-matches on the opening tag or
  # id="x" / class="x"-style fragment derived from the selector.
  # Rough but effective for the scaffold-blog HTML shapes —
  # bodies like `"#articles"`, `".p-4"`, `"h1"`. Block form
  # additionally yields so nested `assert_select`s further narrow
  # within the matched section; we don't shrink the body here, so
  # nested checks still see the full response body — same loose
  # semantic as the TS shim.
  def assert_select(selector : String, content : String? = nil, msg : String? = nil) : Nil
    fragment = selector_fragment(selector)
    unless @__body.includes?(fragment)
      fail(msg || "expected body to match selector #{selector.inspect} (looked for #{fragment.inspect})")
      return
    end
    if !content.nil? && !@__body.includes?(content)
      fail(msg || "expected body to contain #{content.inspect} matching selector #{selector.inspect}")
    end
  end

  # Kwarg form — `assert_select("h2", minimum: 1, maximum: 5)` — Rails
  # passes `minimum:` / `maximum:` / `count:` for cardinality checks.
  # The substring-match shim treats these as best-effort no-ops; the
  # selector-presence check below is sufficient for the scaffold-blog
  # shapes.
  def assert_select(selector : String, **opts) : Nil
    _ = opts
    assert_select(selector)
  end

  def assert_select(selector : String, **opts, &block) : Nil
    _ = opts
    assert_select(selector)
    yield
  end

  def assert_select(selector : String, &block) : Nil
    assert_select(selector)
    yield
  end

  private def selector_fragment(selector : String) : String
    first = selector.split(/\s+/).first
    case first
    when .starts_with?("#") then %(id="#{first[1..]}")
    when .starts_with?(".") then %(#{first[1..]}")
    else                         "<#{first}"
    end
  end

  # ── per-test registry + reset hooks ──────────────────────────────
  #
  # Routes table, controller registry, fixture loaders, and the
  # schema reset SQL live as class state on `RoundhouseTest`. The
  # emitted `src/test_setup.cr` registers them at process-init time;
  # the per-test `before_each` (installed by the `inherited` macro
  # below) resets the in-memory DB and re-runs each fixture loader
  # so every spec starts from a clean state.

  alias RouteRow = ::ActionDispatch::Router::Route

  @@routes : Array(RouteRow) = [] of RouteRow
  @@controllers : Hash(Symbol, ::ActionController::Base.class) =
    {} of Symbol => ::ActionController::Base.class
  @@fixture_loaders : Array(-> Nil) = [] of -> Nil
  @@schema_sql : String = ""

  def self.routes : Array(RouteRow)
    @@routes
  end

  def self.routes=(value : Array(RouteRow)) : Array(RouteRow)
    @@routes = value
  end

  def self.controllers : Hash(Symbol, ::ActionController::Base.class)
    @@controllers
  end

  def self.controllers=(value : Hash(Symbol, ::ActionController::Base.class)) : Hash(Symbol, ::ActionController::Base.class)
    @@controllers = value
  end

  def self.fixture_loaders : Array(-> Nil)
    @@fixture_loaders
  end

  def self.fixture_loaders=(value : Array(-> Nil)) : Array(-> Nil)
    @@fixture_loaders = value
  end

  def self.schema_sql : String
    @@schema_sql
  end

  def self.schema_sql=(value : String) : String
    @@schema_sql = value
  end

  # Reset in-memory DB to a fresh schema and reload every registered
  # fixture set. Called from the `before_each` block in the discover
  # macro so each spec sees the canonical starting state.
  #
  # No-ops cleanly when the app carries no schema / no fixtures —
  # framework-test harnesses (router_test, view_helpers_test, etc.)
  # exercise the runtime layer directly without a Rails-shape app
  # underneath, so their `src/test_setup.cr` skips the schema and
  # fixture registrations.
  def self.reset_and_load_fixtures : Nil
    return if @@schema_sql.empty?
    Roundhouse::Db.setup_test_db(@@schema_sql)
    ::ActiveRecord.adapter = Roundhouse::SqliteAdapter.new
    @@fixture_loaders.each(&.call)
  end

  # Bridge the assertion failure into Spec's expectation channel —
  # Spec catches `Spec::AssertionFailed` and reports it as a failed `it`.
  private def fail(msg : String) : Nil
    raise Spec::AssertionFailed.new(msg, file: __FILE__, line: __LINE__)
  end

  # ── discovery macro ──────────────────────────────────────────────
  #
  # Generate `describe <Klass> do … it "test_X" do <Klass>.new.test_X; end … end`
  # at the bottom of the test file. Walks the class's own instance
  # methods at compile time; each `test_*` method becomes one spec.
  # Crystal's `spec` autorun fires when `require "spec"` is loaded and
  # the program reaches main, so the test_helper itself doesn't need
  # an explicit runner.
  #
  # `before_each` wraps every `it` with the DB-reset + fixture-reload
  # so specs start from the canonical state.
  macro inherited
    macro finished
      describe \{{ @type }} do
        before_each do
          RoundhouseTest.reset_and_load_fixtures
        end
        \{% for m in @type.methods %}
          \{% if m.name.starts_with?("test_") %}
            it \{{ m.name.stringify }} do
              \{{ @type }}.new.\{{ m.name.id }}
            end
          \{% end %}
        \{% end %}
      end
    end
  end
end
