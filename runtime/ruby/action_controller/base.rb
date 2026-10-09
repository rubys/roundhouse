require_relative "../action_dispatch/flash"
require_relative "../action_dispatch/session"
require_relative "../action_view"

module ActionController
  # One-slot array so class-level CSRF state is a store every target
  # can index, not a `self` ivar or `class << self` writer.
  #
  # Default OFF: extras transpile this file without
  # `authenticity_token.rb` (empty `masked_authenticity_token` stub),
  # so fail-closed CSRF turns every POST into 422. The ruby-family
  # reopen in authenticity_token.rb flips the flag on when the real
  # masked-token implementation is present. Tests that need the check
  # set `allow_forgery_protection = true` explicitly.
  FORGERY_SLOT = [false]

  def self.forgery_flag
    FORGERY_SLOT[0] == true
  end

  def self.set_forgery_flag(value)
    FORGERY_SLOT[0] = value
  end

  # Empty until `authenticity_token.rb` reopens these: strict-target
  # emit of this file must not call `Current.session` or XOR bytes.
  # Strict targets therefore issue no token and check none (see
  # docs/guide/rails-coverage.md): an empty session secret means
  # "no minting on this lane," not "fail closed."
  def self.masked_authenticity_token
    ""
  end

  def self.csrf_token_valid?(given, expected)
    return true if expected.empty?
    given.length > 0 && given == expected
  end

  # WHATWG URL-parser preprocessing: drop tab/CR/LF/NUL anywhere, then
  # strip leading and trailing C0 controls and spaces. A tab in the
  # middle (`/\t/evil`) becomes `//evil` so host classification sees it.
  REDIRECT_LINE_BREAKS = { "\r" => "", "\n" => "", "\0" => "", "\t" => "" }.freeze
  REDIRECT_LINE_BREAK_PATTERN = /[\r\n\0\t]/.freeze

  # Puma's illegal-header rule: drop a key/value that cannot be one
  # HTTP/1.1 line. Keys scan UTF-8 bytes; values walk characters and
  # recognize the CRLF grapheme. CRuby/JRuby replace these portable
  # predicates with regex checks in the target overlay.
  def self.header_key_ok?(k)
    return false if k.nil?
    # Delimiters can share a Swift grapheme with a combining mark. Inspect
    # their UTF-8 bytes so the policy does not depend on string indexing.
    bytes = k.bytes
    n = bytes.length
    return false if n == 0
    i = 0
    while i < n
      byte = bytes[i]
      return false if byte <= 32 || byte == 34 || byte == 58 || byte == 127
      i += 1
    end
    true
  end

  def self.header_value_ok?(v)
    # Nil is an unset (`headers["X-Rev"] = ENV["GIT_REVISION"]` when
    # the env is absent). Drop it; do not ask it for length.
    return false if v.nil?
    n = v.length
    i = 0
    while i < n
      c = v[i, 1].to_s
      return false if c != "\t" && header_control?(c)
      i += 1
    end
    true
  end

  def self.header_control?(c)
    # Swift strings index grapheme clusters: CRLF can be one element.
    return true if c == "\r\n"
    c == "\0" || c == "\r" || c == "\n" || c == "\x01" || c == "\x02" ||
      c == "\x03" || c == "\x04" || c == "\x05" || c == "\x06" || c == "\x07" ||
      c == "\x08" || c == "\t" || c == "\x0b" || c == "\x0c" || c == "\x0e" ||
      c == "\x0f" || c == "\x10" || c == "\x11" || c == "\x12" || c == "\x13" ||
      c == "\x14" || c == "\x15" || c == "\x16" || c == "\x17" || c == "\x18" ||
      c == "\x19" || c == "\x1a" || c == "\x1b" || c == "\x1c" || c == "\x1d" ||
      c == "\x1e" || c == "\x1f" || c == "\x7f"
  end

  def self.sanitize_location(path)
    s = path.to_s
    if s.include?("\r") || s.include?("\n") || s.include?("\0") || s.include?("\t")
      s = s.gsub(REDIRECT_LINE_BREAK_PATTERN, REDIRECT_LINE_BREAKS)
    end
    s = s.tr("\\", "/")
    # Index-walk strip helpers (one counter while each). Not
    # `while keep && …` (Elixir BoolOp) and not two whiles in this
    # method (while_to_recursion allows one top-level while per method).
    s = strip_leading_controls(s)
    s = strip_trailing_controls(s)
    s
  end

  def self.strip_leading_controls(s)
    # Index walk — not per-char recursion (stack-safe on long pads).
    # Canonical counter `while` so Elixir while_to_recursion applies:
    # early return when a kept char is found; trailing `i += 1` step.
    n = s.length
    i = 0
    while i < n
      c = s[i, 1].to_s
      if !(c == " " || header_control?(c))
        return s[i, n - i].to_s
      end
      i += 1
    end
    ""
  end

  def self.strip_trailing_controls(s)
    n = s.length
    i = n
    while i > 0
      c = s[i - 1, 1].to_s
      if !(c == " " || header_control?(c))
        return s[0, i].to_s
      end
      i -= 1
    end
    ""
  end

  # Host of an absolute URL (`http://h/path`), or "" when the value is
  # a relative path. Protocol-relative `//host/...` is a host. A
  # backslash or interior tab is normalized in `sanitize_location`
  # first so `/\evil` and `/\t/evil` become `//evil`.
  def self.location_host(url)
    s = url.to_s
    return "" if s.empty?
    rest = s
    if s.start_with?("//")
      rest = s[2, s.length].to_s
      # `///path` has no host; treat as a dummy host so same-host
      # refuses it instead of classifying it as relative.
      return "." if rest.empty? || rest.start_with?("/")
    else
      at = find_substr(s, "://")
      return "" if at < 0
      rest = s[at + 3, s.length].to_s
    end
    slash = find_substr(rest, "/")
    hostport = slash < 0 ? rest : rest[0, slash].to_s
    q = find_substr(hostport, "?")
    hostport = hostport[0, q].to_s unless q < 0
    hash = find_substr(hostport, "#")
    hostport = hostport[0, hash].to_s unless hash < 0
    user = find_last(hostport, "@")
    hostport = hostport[user + 1, hostport.length].to_s unless user < 0
    hostport.downcase
  end

  def self.find_substr(hay, needle)
    n = needle.length
    i = 0
    last = hay.length - n
    while i <= last
      return i if hay[i, n].to_s == needle
      i += 1
    end
    -1
  end

  def self.find_last(hay, needle)
    n = needle.length
    i = hay.length - n
    while i >= 0
      return i if hay[i, n].to_s == needle
      i -= 1
    end
    -1
  end

  class HeaderStore
    def initialize
      @keys = []
      @vals = []
    end

    def [](key)
      i = 0
      while i < @keys.length
        return @vals[i] if @keys[i] == key
        i += 1
      end
      nil
    end

    # Void: a writer that returns the stored value would leak a
    # dropped line back to the caller, and rust emit of `[]=` is
    # `()` not `Option`.
    def []=(key, value)
      if ActionController.header_key_ok?(key) && ActionController.header_value_ok?(value)
        i = 0
        found = false
        while i < @keys.length
          if @keys[i] == key
            @vals[i] = value
            found = true
          end
          i += 1
        end
        unless found
          @keys << key
          @vals << value
        end
      end
    end

    def size
      @keys.length
    end

    def key_at(i)
      @keys[i].to_s
    end

    def val_at(i)
      @vals[i].to_s
    end
  end

  # Symbol form (`status: :see_other`) to integer code — Rack's
  # `SYMBOL_TO_STATUS_CODE`, PORTED whole rather than grown entry by
  # entry as apps surfaced them. It used to be "an ad-hoc subset; grow
  # as new statuses surface", and campfire showed what that costs:
  # `head :too_many_requests` in its ban filter missed the table, fell
  # through to the 200 default, and its rate-limit tests asserted a 429
  # against a silent success. A registry with a fixed, published
  # membership is not something to derive from the corpus one miss at a
  # time — see the same argument about inflections in the compiler.
  STATUS_CODES = {
    continue:                        100,
    switching_protocols:             101,
    processing:                      102,
    early_hints:                     103,
    ok:                              200,
    created:                         201,
    accepted:                        202,
    non_authoritative_information:   203,
    no_content:                      204,
    reset_content:                   205,
    partial_content:                 206,
    multi_status:                    207,
    already_reported:                208,
    im_used:                         226,
    multiple_choices:                300,
    moved_permanently:               301,
    found:                           302,
    see_other:                       303,
    not_modified:                    304,
    use_proxy:                       305,
    temporary_redirect:              307,
    permanent_redirect:              308,
    bad_request:                     400,
    unauthorized:                    401,
    payment_required:                402,
    forbidden:                       403,
    not_found:                       404,
    method_not_allowed:              405,
    not_acceptable:                  406,
    proxy_authentication_required:   407,
    request_timeout:                 408,
    conflict:                        409,
    gone:                            410,
    length_required:                 411,
    precondition_failed:             412,
    payload_too_large:               413,
    content_too_large:               413,
    uri_too_long:                    414,
    unsupported_media_type:          415,
    range_not_satisfiable:           416,
    expectation_failed:              417,
    misdirected_request:             421,
    # Rails 8.1.x scaffold renamed `:unprocessable_entity` →
    # `:unprocessable_content` mid-version. Alias both so emit follows
    # whichever the fixture's scaffold currently produces.
    unprocessable_entity:            422,
    unprocessable_content:           422,
    locked:                          423,
    failed_dependency:               424,
    too_early:                       425,
    upgrade_required:                426,
    precondition_required:           428,
    too_many_requests:               429,
    request_header_fields_too_large: 431,
    unavailable_for_legal_reasons:   451,
    internal_server_error:           500,
    not_implemented:                 501,
    bad_gateway:                     502,
    service_unavailable:             503,
    gateway_timeout:                 504,
    http_version_not_supported:      505,
    variant_also_negotiates:         506,
    insufficient_storage:            507,
    loop_detected:                   508,
    bandwidth_limit_exceeded:        509,
    not_extended:                    510,
    network_authentication_required: 511,
  }.freeze

  # Base controller class. Holds the per-request state (params,
  # session, flash) and the response state (status, body, location).
  # Subclasses define their actions and a `process_action` dispatch
  # case (since spinel forbids `send` with non-literal symbols, the
  # action dispatch has to be explicit per-controller).
  #
  # NOTE: `cookies` is intentionally NOT here. It's a CRuby-target
  # feature (used by lobsters, not the blog) provided via the Ruby
  # overlay (runtime/action_controller_cookies.rb), so the shared
  # runtime stays target-agnostic — a CookieJar in this transpiled
  # file would have to satisfy every strict target's type system for
  # a feature none of them exercise yet.
  class Base
    def self.allow_forgery_protection
      ActionController.forgery_flag
    end

    def self.allow_forgery_protection=(value)
      ActionController.set_forgery_flag(value)
    end

    attr_accessor :params, :session, :flash, :request_method, :request_path, :request_format
    # Raw query string (no leading `?`), as the request carried it. Path-
    # option redirects that keep the query read this; dispatchers set it
    # alongside `request_path` so every target sees the same value.
    attr_accessor :query_string
    # True when the request's Accept is a bare `*/*` — an
    # XMLHttpRequest or fetch that set none. Rails reads that as "any
    # format", so an action with no html template renders the template
    # it does have (see `html_fallback` in lower::controller::body).
    # The dispatcher sets it; false keeps every other request on the
    # html path.
    attr_accessor :accepts_any_format
    # Rails' `request.path_parameters`: the matched route's own segments
    # (`{"length" => "1y"}` on `/top/1y`), not the query string. The
    # dispatcher assigns it; a `url_for` options hash reads it to fill a
    # segment the hash leaves out, as Rails recalls it.
    attr_accessor :path_parameters
    # Rails' `action_name`, as a String. The synthesized `process_action`
    # sets it only in a controller that reads it.
    attr_reader   :action_name
    attr_reader   :status, :body, :location, :content_type
    # Cache-Control, split into two TYPED readers rather than Rails'
    # one mixed Hash. Rails' `response.cache_control` is
    # `{public: true, max_age: 31556952}` — an Integer and a boolean in
    # one container, which is the type bag every strict target pays
    # for. The two facts are kept apart here and re-assembled into
    # Rails' Hash shape by the TEST harness
    # (ActionResponse#cache_control), the only reader that wants the
    # subscript spelling.
    attr_reader   :cache_control_max_age, :cache_control_public

    # `response` is this controller in the shared runtime, so controller
    # actions that set `response.content_type` need the same writer Rails'
    # response object exposes.
    def content_type=(value)
      @content_type = value
      @content_type
    end

    def initialize
      @params  = {}
      @path_parameters = {}
      @action_name = ""
      @session = ActionDispatch::Session.new
      @flash   = ActionDispatch::Flash.new
      @status  = 200
      @body    = +""
      @location = nil
      @request_method = +""
      @request_path = +""
      @request_format = :html
      @query_string = +""
      @accepts_any_format = false
      @content_type = "text/html; charset=utf-8"
      @headers = ActionController::HeaderStore.new
      @performed = false
      # Set unconditionally, not on first `expires_in`: an ivar a strict
      # target never sees assigned has no type to infer, and the readers
      # above are reachable on every controller. 0 = "no max-age
      # stated", which is also what a response without the header means.
      @cache_control_max_age = 0
      @cache_control_public = false
    end

    # True once render/redirect_to/head has produced a response.
    # The synthesized `process_action` filter preamble checks this
    # after each before_action that can render or redirect — Rails'
    # halting semantics: a filter that responds skips the action.
    def performed?
      @performed
    end

    # Discard the current session (Rails' logout idiom). The dispatch
    # layer persists whatever the session holds after the action; an
    # empty replacement means the outbound session cookie is cleared
    # (or, when a CSRF token is lazily re-added during render, that
    # the next session starts fresh — matching Rails' new-session-id
    # semantics closely enough for cookie-carried state).
    #
    # The trailing `@session` read is load-bearing: assignment is not
    # a value expression on the strict targets (kotlin/swift/C#/rust
    # all reject a `-> Session` body ending in an assignment), so the
    # return must be an explicit read. Same rule as the CookieJar
    # cascade — mutation methods with non-void returns.
    def reset_session
      @session = ActionDispatch::Session.new
      @session
    end

    # The dispatcher's seat for the inbound cookie session.
    #
    # NOT `controller.session = …`, and the reason is a name collision
    # the app owns. `Main.instantiate_controller` returns one of N
    # controllers, which spinel represents as a fully-poly value rather
    # than a union of those N — so a `session=` send there dispatches
    # over EVERY class in the program defining that name. campfire's
    # `class Current < ActiveSupport::CurrentAttributes` declares
    # `attribute :session`, and ITS `session=` takes the app's `Session`
    # RECORD (it reads `session.user`). The switch therefore had an arm
    # passing an `ActionDispatch::Session` into a `Session *`, and the C
    # build stopped.
    #
    # A name only the framework writes sidesteps it: every arm of this
    # dispatch is a controller, and they all store the same type.
    # `request=`/`user=` collide with the same `Current` attributes and
    # do NOT fail only because those types happen to agree today.
    def assign_http_session(value)
      @session = value
      @session
    end

    # The dispatcher's seat for `action_name`. The router gives
    # `process_action` a Symbol, and Rails gives the action a String,
    # so this method converts it. A framework-only name avoids the
    # `name=` collision that `assign_http_session` describes.
    def assign_action_name(name)
      @action_name = name.to_s
      @action_name
    end

    # Rails' `controller_name` / `controller_path`: the demodulized
    # underscored leaf (`ArticlesController` → `"articles"`) and the
    # path form that keeps namespaces (`Admin::UsersController` →
    # `"admin/users"`). Defaults answer for `ActionController::Base`
    # itself. Each concrete controller's lowerer overrides both with
    # string literals — AOT targets cannot host `self.class.to_s`
    # reflection, and a shared ActiveSupport char-walk (`underscore`
    # / `demodulize`) does not yet compile on every strict-target
    # string emit.
    def controller_name
      "base"
    end

    def controller_path
      "action_controller/base"
    end

    # Subclasses override. Error message omits `self.class.name` —
    # `.name`-style reflection forks across targets and the runtime
    # stack trace already identifies the receiver's class.
    def process_action(_action_name)
      raise NotImplementedError, "process_action must be overridden by subclass"
    end

    # Render a response. The `content_type` kwarg defaults to the
    # current `@content_type` (`text/html; charset=utf-8` on init).
    # Jbuilder-lowered actions pass `content_type: "application/json"`
    # on the JSON branch; the html branch omits it and rides the
    # default. The `location:` kwarg sets @location so the CGI driver
    # ships a Location header alongside the rendered body — Rails'
    # `render :show, status: :created, location: @article` idiom for
    # POST 201 responses. Distinct from redirect_to (which uses a 3xx
    # status); main.rb dispatches on status, not on @location nil-ness.
    def render(body, status: :ok, content_type: nil, location: nil)
      @body   = body
      @status = resolve_status(status)
      @performed = true
      @content_type = content_type unless content_type.nil?
      @location = ActionController.sanitize_location(location) unless location.nil?
      nil
    end

    # `redirect_to(path, notice:, alert:, status:)` — sets location +
    # status; surfaces flash messages via the flash hash. Default
    # status 302 (Found). Real-blog uses 303 (See Other) on
    # PATCH/DELETE responses; pass `status: :see_other` to match.
    #
    # CR, LF and NUL are DELETED from the location, as Rails'
    # `_compute_redirect_to_location` does (`.delete("\0\r\n")`).
    # An absolute URL whose host is not this request's Host is refused
    # (`raise_on_open_redirects`): `redirect_to params[:back]` and a
    # stored `request.url` from a spoofed `HTTP_HOST` must not become
    # Location on another origin.
    def redirect_to(path, notice: nil, alert: nil, status: :found)
      loc = ActionController.sanitize_location(path)
      loc = same_host_location(loc)
      @location = loc
      @status   = resolve_status(status)
      @performed = true
      @flash[:notice] = notice unless notice.nil?
      @flash[:alert]  = alert  unless alert.nil?
      nil
    end

    # `head(:no_content, content_type: "application/json")` — empty
    # body, status only. The `content_type` kwarg is set by the
    # respond_to-flattener's JSON branch when it preserves a
    # `head :sym` terminal; html branches omit it and the default
    # text/html stands. (Body-empty responses make Content-Type
    # mostly irrelevant per RFC 7230, but some HTTP clients still
    # parse it, so being explicit costs nothing.)
    # `location:` is Rails' own option and campfire's bot create writes
    # it (`head :created, location: message_url(@message)`) — a 201 that
    # names the resource it made. It is NOT a redirect: `redirect?` gates
    # on a 3xx status, so setting the location beside a 201 records the
    # URL without turning the response into one.
    def head(status, content_type: nil, location: nil)
      @location = ActionController.sanitize_location(location) unless location.nil?
      @status = resolve_status(status)
      @body   = +""
      @performed = true
      @content_type = content_type unless content_type.nil?
      nil
    end

    # `response.headers["Expires"] = …` — Rails actions reach header
    # state through the response object; this controller IS its own
    # buffered response, so `response` returns self and `headers` the
    # extra-header hash. The CGI harness emits status/body/
    # content-type today; extra headers are buffered but unsent — a
    # ledgered seam (they tune caching, not content), wired through
    # the harness when a consumer needs them.
    def response
      self
    end

    def response_body
      @body
    end

    def response_body=(value)
      @body = value
      @performed = !value.nil?
      @body
    end

    # ---- conditional GET: ALWAYS FRESH -----------------------------
    #
    # Rails' `fresh_when` / `stale?` compare the client's
    # `If-None-Match` / `If-Modified-Since` against an ETag or
    # timestamp and answer 304 on a match. Neither half of that
    # comparison exists here: this controller has no request object
    # (only `@request_format`), so there is nothing to read the
    # conditional headers FROM, and the extra-header hash above is
    # buffered but never sent, so there is nothing to write the
    # validators TO.
    #
    # What IS available is the answer Rails gives when a client sends
    # no conditional header at all: the response is stale, render it.
    # That is this implementation, and it is a subset rather than a
    # stub — always-render is semantically CORRECT for every request,
    # it just never earns the 304. The cost is bandwidth, not
    # behavior, which is why it can ship ahead of the plumbing.
    #
    # Monomorphic on the shapes the corpus writes — `fresh_when
    # @messages` and `stale?(etag: record)`. Rails accepts several more
    # (`fresh_when(etag:, last_modified:)`, `stale?(record)`); each
    # would be its own method here when a call site asks, rather than
    # one method with a union parameter no target can narrow.
    # Empty body, not `nil`: the same no-op idiom
    # `Base.preload_associations` uses. A body that is only a bare `nil`
    # gives Rust an `Option` with nothing to infer its parameter from
    # (`E0282: type annotations needed` on a lone `None;`), where an
    # empty body is a plain `void`.
    # The parameter is NOT named `record`: the Elixir emitter prepends
    # the controller struct as an implicit first argument and calls it
    # `record`, so a same-named parameter of our own emits
    # `def fresh_when(_record, _record)` — a duplicate match variable,
    # which that target's `--warnings-as-errors` build rejects. Every
    # other method here already avoids the name; this one found out why.
    def fresh_when(subject)
    end

    def stale?(etag: nil)
      true
    end

    # `expires_in 1.year, public: true` — Rails' Cache-Control writer.
    # campfire's QR code, avatar and logo actions all open with one, and
    # the QR test reads the result back
    # (`response.cache_control[:max_age]`), which is what makes this a
    # VALUE the harness carries rather than a header it would be enough
    # to buffer.
    #
    # SECONDS, not a Duration. `lower::duration`'s `rewrite_expires_in`
    # unwraps the corpus' `1.year` at the CALL SITE — the same grounding
    # `signed_id(expires_in:)` gets — so this signature stays Integer
    # and no strict target pays for an `untyped` parameter here.
    # `stale_while_revalidate: 0` means "not stated", which keeps the
    # parameter Integer rather than nullable; two of campfire's three
    # call sites pass it, so accepting only `public:` would have turned
    # a NoMethodError into an ArgumentError at those two and looked like
    # progress.
    #
    # NO HEADER IS WRITTEN. Composing the `Cache-Control` string here
    # and parking it in the buffered-but-unsent `headers` hash above
    # would be work nothing reads — and `@headers[k] = v` does not
    # survive the Rust emitter, which renders a Hash index-assign as
    # `self.headers[k] = v` where `HashMap` wants `.insert()` (E0594:
    # `IndexMut` is not implemented). The two readers hold everything
    # the response needs; the wire spelling is the harness's to compose
    # when it starts emitting headers at all.
    def expires_in(seconds, public: false, stale_while_revalidate: 0)
      @cache_control_max_age = seconds
      @cache_control_public = public
      nil
    end

    def headers
      @headers
    end

    # `send_data data, type:, disposition:` — a binary response body
    # (lobsters streams avatar PNGs). Same buffering contract as
    # render. `disposition` is accepted but not yet buffered — extra
    # headers ride the same unsent seam as `headers` above, and the
    # Content-Disposition write joins it when the harness wires
    # header emission.
    def send_data(data, type: "application/octet-stream", disposition: "attachment")
      @body = data
      @content_type = type
      @performed = true
      disp = disposition.to_s == "inline" ? "inline" : "attachment"
      @headers["Content-Disposition"] = disp
      nil
    end

    # Rails' `verify_authenticity_token`: GET/HEAD pass; anything else
    # must carry a token that unmasks to the session secret, as
    # `authenticity_token` or `X-CSRF-Token`. An empty session token
    # matches nothing (fail closed). Tests set
    # `allow_forgery_protection = false`.
    #
    # No trailing `nil`: Elixir's mutation-threaded emit would assign
    # the `unless`/`render` result to `record` and then discard it,
    # tripping `--warnings-as-errors` on an unused variable.
    def verify_authenticity_token
      unless verified_request?
        render "<h1>422 Unprocessable Content</h1>", status: :unprocessable_content
      end
    end

    def verified_request?
      return true unless ActionController.forgery_flag
      verb = @request_method.to_s
      return true if verb == "" || verb == "GET" || verb == "HEAD"
      # Nil-then-`to_s` lives in `csrf_session_secret` so the secret is
      # a typed String at this call (Rust `csrf_token_valid?` takes
      # `&str`; an Untyped local would pass an owned String).
      token = params.fetch("authenticity_token", "")
      return true if ActionController.csrf_token_valid?(token.to_s, csrf_session_secret)
      ActionController.csrf_token_valid?(csrf_header_token, csrf_session_secret)
    end

    # Nil-then-`to_s` — not `session[:k].to_s` alone. Strict-target
    # emit turns a missing key into JS `undefined`, and `String(undefined)`
    # is `"undefined"`, which would fail closed even when no secret was
    # minted. The early `nil?` keeps an absent secret as `""` so the
    # stub `csrf_token_valid?` can check-none; ruby-family
    # `AuthenticityToken.valid?` still fails closed on empty.
    # `nil?` / `to_s` stay on the index send: Rust Session `#[]` is
    # `Option<String>`.
    def csrf_session_secret
      return "" if session[:_csrf_token].nil?
      session[:_csrf_token].to_s
    end

    def csrf_header_token
      ""
    end

    def request_for_csrf
      nil
    end

    # Relative locations (`/path`, not `//host`) pass. An absolute URL
    # must name this request's host; a missing request refuses any host.
    def same_host_location(loc)
      host = ActionController.location_host(loc)
      return loc if host.empty?
      req_host = request_host_for_redirect
      if req_host.empty? || host != req_host.downcase
        raise ArgumentError, "Unsafe redirect to \"" + loc + "\", pass allow_other_host: true to redirect anyway."
      end
      loc
    end

    def request_host_for_redirect
      ""
    end

    # Monomorphic on Symbol — real-blog never passes a literal Integer
    # status, so the previous `is_a?(Integer)` pass-through branch is
    # contracted away. Symbol -> Integer via the STATUS_CODES table.
    #
    # RAISES on a symbol the table doesn't carry, as Rails does
    # ("Invalid HTTP status"). It used to answer 200, which turned a
    # missing table entry into a response that LOOKED like success:
    # campfire's ban filter halted the chain with the right intent and
    # the wrong code, and every assertion downstream of it read a
    # perfectly ordinary 200. With the table now ported whole, a miss
    # means the app named a status no HTTP registry has.
    def resolve_status(s)
      raise "Invalid HTTP status: #{s}" unless STATUS_CODES.key?(s)
      STATUS_CODES.fetch(s, 200)
    end
  end
end
