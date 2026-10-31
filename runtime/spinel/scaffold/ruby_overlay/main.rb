# frozen_string_literal: true
# Top-level entry point for the CRuby (Puma/Rack) target.
#
# Dispatch lives in `Main.dispatch_core`, which returns a response
# descriptor; two wrappers serialize it:
#   * `Main.run_rack(env)`         — a Rack `[status, headers, [body]]`
#                                    tuple; the Puma serving path
#                                    (config.ru) calls this.
#   * `Main.run(env, stdin, stdout)` — a CGI byte stream on stdout; the
#                                    one-shot script path (below) and the
#                                    view/controller tests use this.
#
# Library usage (from tests):
#   require_relative "main"
#   Main.run(env_hash, body_io, response_io)   # CGI bytes to response_io
#
# Script usage (one-shot CGI):
#   REQUEST_METHOD=GET PATH_INFO=/articles ruby main.rb
# Or behind a CGI-aware server:
#   AddHandler cgi-script .rb       (apache)
#   alias /blog /path/to/main.rb    (nginx + fcgiwrap)


# SqliteAdapter is hoisted to top-level so the spinel-AOT compile
# can statically resolve the `SqliteAdapter` constant referenced
# from `Base#save` etc. via the adapter dispatcher. Under CRuby the
# require is harmless (the gem-backed shim only opens a DB on
# `configure`); under spinel the FFI-backed shim only emits when
# `runtime/sqlite_adapter` is in the require graph.
# The whole require chain (see boot.rb). Kept in its own file so
# `test/test_helper.rb` can load the app without loading this one,
# which starts a server.
require_relative "boot"

module Main
  # The framework class used by the nominal ParseError fallback.
  FrameworkParseError = ActionDispatch::Http::Parameters::ParseError

  # Dispatch one request to a response descriptor — the single source
  # of routing / controller / flash / redirect logic. Returns the
  # 10-tuple `[status, body, content_type, location, set_cookies,
  # extra_headers, secure_cookies, samesite_cookies, httponly_cookies,
  # expires_cookies]`
  # (the first six are the exact argument shape `CgiIo.write_response`
  # consumes; the rest are the explicit cookie-flag maps `run_rack`
  # needs). Two thin wrappers sit on top:
  # `run` (CGI byte stream — tests + one-shot script mode) and
  # `run_rack` (a Rack tuple — the Puma serving path), so neither the
  # CGI string nor the Rack hash is the canonical form and the dispatch
  # body lives exactly once.
  #
  # `ActionView::ViewHelpers` and `ActionDispatch::Router` are
  # written fully-qualified (rather than the prior `include
  # ActionView`/`include ActionDispatch` + bare names) so spinel-AOT's
  # constant resolver sees the references without walking included-
  # module namespaces — a path it doesn't currently follow.

  # The composed dispatch table, built once. `RouteTable.table` is an
  # emitted def that CONSTRUCTS every Route object on each call —
  # rebuilding ~200 of them per request (~23k allocations per bench
  # iteration) is pure GC pressure for boot-time constants. At today's
  # baseline the ms/iter delta is inside run noise; this is allocation
  # hygiene, not a measured win. (Benign race under Puma threads: both
  # winners compute the same array.)
  def self.route_table
    @route_table ||= [RouteTable.root] + RouteTable.table + ActiveStorage::Routes.table
  end

  # A 404 the app did not rescue (no route, ActionNotFound,
  # RecordNotFound), answered as Rails' production exceptions app
  # (ActionDispatch::PublicExceptions) does: `{"status":404,"error":
  # "Not Found"}` when the request's format is JSON
  # (ActionDispatch::MimeNegotiation.json_format?), the HTML page
  # otherwise. The spinel twin is `Main.render_not_found`.
  def self.not_found_response(format_param, path, request, env)
    extension = path.end_with?(".json") ? "json" : ""
    if ActionDispatch::MimeNegotiation.json_format?(format_param, extension,
         request.fetch(:accept, "").to_s, env["HTTP_X_REQUESTED_WITH"].to_s, env["CONTENT_TYPE"].to_s)
      return [404, %({"status":404,"error":"Not Found"}), "application/json; charset=UTF-8", nil, {}, {}, {}, {}, {}]
    end
    [404, "<h1>404 Not Found</h1>", "text/html; charset=utf-8", nil, {}, {}, {}, {}, {}]
  end

  def self.dispatch_core(env, stdin)
    # Rails wraps every request in the AR query cache: identical
    # SELECTs within one request replay the first result; any write
    # invalidates. The CRuby Db shim implements the same discipline
    # (fiber-local, so Puma threads don't share entries).
    Db.query_cache_begin
    # A GET/HEAD reads through one SQLite snapshot (Db.read_snapshot_begin).
    # Any write still works: the shim ends the snapshot before it.
    method = env["REQUEST_METHOD"]
    snapshot = method == "GET" || method == "HEAD"
    Db.read_snapshot_begin if snapshot
    begin
      dispatch_core_inner(env, stdin)
    ensure
      Db.read_snapshot_end if snapshot
      Db.query_cache_end
    end
  end

  # Decode a Rack request and dispatch its complete path through Router,
  # preserving literal suffix routes while negotiating the response format.
  def self.dispatch_core_inner(env, stdin)
    ActionView::ViewHelpers.reset_slots!
    Broadcasts.reset_log!
    # config.assume_ssl, same as the spinel dispatcher: the Rack env is
    # what `request.ssl?` and the Secure cookie flag both read.
    if AppEnvironment.serving_assume_ssl?
      env["HTTPS"] = "on"
      env["HTTP_X_FORWARDED_PROTO"] = "https"
      env["rack.url_scheme"] = "https"
    end

    request = CgiIo.parse_request(env, stdin)
    # Browser forms can only GET/POST; a hidden `_method` field carries
    # the real verb (PATCH/DELETE) — the same override Rails' middleware
    # stack performs. Done here, after the body parse, so every serving
    # shape (CGI, Rack, spinel's sphttp) honors it; Rack::MethodOverride
    # can't be used because it consumes the non-rewindable rack.input.
    if request[:method] == "POST"
      override = request[:params]["_method"].to_s.upcase
      if override == "PUT" || override == "PATCH" || override == "DELETE"
        request[:method] = override
      end
    end
    # Infer the response format while retaining the full request path.
    # Router.match owns suffix matching and the literal-path fallback;
    # stripping here would make an explicit `/feed.json` route unreachable.
    request_format = :html
    request_path = request[:path]
    if request_path.end_with?(".json")
      request_format = :json
    end
    # Turbo Stream is negotiated by the Accept header, not by a path
    # suffix — a Turbo-driven form POST asks for
    # `text/vnd.turbo-stream.html`. Checked after the suffix so an
    # explicit `.json` still wins.
    if request_format == :html &&
       request.fetch(:accept, "").to_s.include?("text/vnd.turbo-stream.html")
      request_format = :turbo_stream
    end
    # Prepend ROOT so a GET / request matches before falling through
    # to TABLE. ROOT is kept as a separate constant in routes.rb for
    # legibility (it's the only literal-pattern entry); the dispatch
    # composes them here so Router.match stays a flat-table walk.
    matched = ActionDispatch::Router.match(request[:method], request_path,
                           route_table)
    if matched.nil?
      # No route: the format is the `?format=` param, a trusted Accept,
      # or the path's extension (Main.not_found_response).
      return not_found_response(request[:params]["format"].to_s, request_path, request, env)
    end
    # A `(.:format)` EXTENSION the router stripped off the path
    # (`/rooms/3/refresh.turbo_stream`). The `.json` sniff above runs
    # BEFORE matching and so never sees any other extension; the router
    # captures them all into `path_params["format"]`, and until now
    # nothing read it — every `format: :turbo_stream` URL dispatched as
    # :html and fell through to MissingTemplate.
    #
    # Compared against string literals rather than converted with
    # `to_sym`: the same reason the `req_format` block below names its
    # formats one at a time — a Symbol materialized from a runtime
    # String is a shape the strict targets do not share.
    path_format = matched.path_params.fetch("format", "")
    request_format = :json if path_format == "json"
    request_format = :turbo_stream if path_format == "turbo_stream"
    request_format = :rss if path_format == "rss"
    request_format = :xml if path_format == "xml"
    # `/service-worker.js`: campfire's raw service-worker template.
    request_format = :js if path_format == "js"
    # A route's default format (`get "/rss" => "home#index", :format =>
    # "rss"`, `defaults: { format: :json }`) fills in what the request
    # left out, as in Rails: it applies only when the path carried no
    # extension, and is then written to `params[:format]` too (as a
    # String, which the action reads). `/widgets/1.html` keeps html on a
    # route defaulting to json.
    if path_format.empty? && !matched.req_format.nil?
      request_format = matched.req_format
      matched.path_params["format"] = matched.req_format.to_s
    end

    # `--target futamura`'s whole-route bridge serves only the actions
    # the analysis cleared (src/emit/futamura/routes.rb); any other
    # match is declined here, before any app code runs, and the host
    # Rails app serves the request. Matched once, here, not again in
    # the bridge.
    allowed = env["roundhouse.actions"]
    if !allowed.nil? && !allowed.include?("#{matched.controller}##{matched.action}")
      env["roundhouse.declined"] = true
      return [404, "", "text/html; charset=utf-8", nil, {}, {}, {}, {}, {}]
    end

    # A JSON body that does not parse: Rails' params parser raises
    # ParseError and the request is answered 400 before any action runs
    # (the spinel twin: `Main.request_params` returning nil).
    if request[:bad_request]
      return [400, "<h1>400 Bad Request</h1>", "text/html; charset=utf-8", nil, {}, {}, {}, {}, {}]
    end

    controller = Main.instantiate_controller(matched.controller)
    merged = matched.path_params.dup
    request[:params].each { |k, v| merged[k] = v }
    controller.params  = merged
    controller.path_parameters = matched.path_params

    # Decode inbound flash from cookies. Each flash key carries via
    # its own cookie (`flash_notice`, `flash_alert`) so the cookie
    # plumbing stays format-free.
    cookies = request[:cookies] || {}
    # Cookie-carried session: restore the whole session from the session
    # cookie (url-encoded k=v pairs; empty when absent or garbled —
    # "logged out", never a 500). The raw inbound value is kept so the
    # persist step below can skip Set-Cookie when the action left the
    # session untouched.
    #
    # The NAME comes from `Rails.application.session_cookie_key` (see the
    # spinel main.rb twin): apps set it with `config.session_store
    # :cookie_store, key: "..."`, and app code that reads the same
    # accessor — lobsters' `remove_unknown_cookies`, which deletes every
    # cookie whose key isn't the configured one — has to agree with the
    # dispatch or the session is cleared on every request.
    # `.to_sym` on the read: CgiIo.parse_cookies keys the inbound hash by
    # Symbol (`out[name.to_sym] = val`). The write side below stays a
    # String — set_cookies keys are only ever interpolated into the
    # header, and `controller.cookies.pending` already contributes String
    # keys to the same hash.
    session_cookie = Rails.application.session_cookie_key
    # SIGNED — see the spinel main.rb twin: `session_in` is the restored
    # session's plain encoding, and only a changed session is re-signed.
    #
    # Under the futamura bridge the host Rails app owns the session: it
    # arrives already decoded as `roundhouse.session` and leaves as
    # `roundhouse.session.out`, with no cookie signed or verified on
    # this side.
    host_session = env["roundhouse.session"]
    restored = if host_session.nil?
      ActionDispatch::Session.from_signed_cookie(
        cookies[session_cookie.to_sym].to_s, session_cookie)
    else
      ActionDispatch::Session.new(host_session)
    end
    session_in = restored.to_cookie
    controller.assign_http_session(restored)
    # Expose the inbound cookies to the controller as a CookieJar so
    # `cookies[:k]` reads (and `cookies[:k] = v` records writes, surfaced
    # below as Set-Cookie). CookieJar is the CRuby-only overlay class.
    controller.cookies = ActionController::CookieJar.new(cookies)
    # Load inbound flash through the constructor (NOT `flash[:k]=`) so the
    # Flash snapshots these as carried-in; `to_persisted` then sweeps the
    # ones merely displayed (show-once). See ActionDispatch::Flash.
    # SIGNED — see the spinel main.rb twin: a cookie that does not verify
    # is no message (and is still cleared below, as a consumed one is).
    inbound_flash = {}
    host_flash = env["roundhouse.flash"]
    if host_flash.nil?
      # Every `flash_<key>` cookie, not only notice/alert: lobsters' layout shows `flash[:error]` / `[:success]`.
      cookies.each do |cname, cvalue|
        name = cname.to_s
        next unless name.start_with?("flash_")
        value = ActionDispatch::SignedCookie.verified(cvalue.to_s, name)
        inbound_flash[name.delete_prefix("flash_")] = value unless value.empty?
      end
    else
      host_flash.each { |k, v| inbound_flash[k.to_s] = v }
    end
    controller.flash = ActionDispatch::Flash.new(inbound_flash)

    controller.request_method = request[:method]
    controller.request_path   = request[:path]
    controller.request_format = request_format
    # Raw query for path-option redirects that keep it (`query_string` on
    # the controller — every target's dispatcher sets the same slot).
    qs = (env["QUERY_STRING"] || request.fetch(:query_string, "")).to_s
    controller.query_string = qs
    # Rails' "any format": a bare `*/*` Accept (see the spinel
    # dispatcher's `Main.accepts_any_format?`); a browser's list with a
    # comma is read as html.
    accept = request.fetch(:accept, "").to_s
    controller.accepts_any_format = !accept.include?(",") && accept.split(";", 2)[0].to_s.strip == "*/*"
    # Rails' `Vary: Accept` on a render whose format came from the Accept
    # header (runtime/action_dispatch/mime_negotiation.rb), as the spinel
    # dispatcher decides it.
    controller.vary_on_accept = ActionDispatch::MimeNegotiation.should_apply_vary_header?(
      accept, env["HTTP_X_REQUESTED_WITH"].to_s, env["CONTENT_TYPE"].to_s,
      merged.key?("format") || !matched.req_format.nil?)
    # The full request object (CRuby overlay class) — filters read
    # `request.remote_ip` / `request.env` / `request[:format]`. `env.to_h`
    # detaches a plain mutable Hash (callers write scratch keys the real
    # ENV would reject); params delegation gets the same merged hash the
    # controller sees.
    controller.request = ActionDispatch::Request.new(env.to_h, merged)
    controller.request.path_parameters = Main.route_parameters(controller, matched)
    # The body's params alone, for ParamsWrapper (`Params.wrap`).
    controller.request.request_parameters = request[:body_params]
    # Same object, module-reachable — helpers are module functions with
    # no controller context (see ActionController::Current).
    ActionController::Current.request = controller.request
    # Park the controller too: the CSRF token generator (overlay
    # form_authenticity_token) reads the live session through it.
    ActionController::Current.controller = controller

    begin
      # Not the class name built per request: only a start_processing / process_action listener reads it.
      processing = ActiveSupport::Notifications.process_listening? ? ActiveSupport::Notifications.start_processing(controller.class.name, matched.action.to_s) : 0.0
      begin
        controller.process_action(matched.action)
      ensure
        ActiveSupport::Notifications.process_action(controller.class.name, matched.action.to_s, processing) unless processing == 0.0
      end
    rescue ActiveRecord::RecordNotFound, ActionController::RoutingError, AbstractController::ActionNotFound
      # The routed format (extension, route default) is the `format`
      # param; an html one may still name `?format=`.
      format_param = request_format == :html ? merged["format"].to_s : request_format.to_s
      return not_found_response(format_param, request_path, request, env)
    rescue ActionController::ParameterMissing, ActionController::BadRequest,
           ActionDispatch::Http::Parameters::ParseError => error
      # The app's handlers have already run. Rails' rescue_responses map
      # an unhandled ParseError to :bad_request by its exact class name;
      # an application subclass has no default mapping.
      # Bind the built-ins so application instance/class overrides cannot
      # turn a subclass into the framework's exact exception class.
      raise if Kernel.instance_method(:is_a?).bind_call(error, FrameworkParseError) &&
               !Kernel.instance_method(:instance_of?).bind_call(error, FrameworkParseError)
      return [400, "<h1>400 Bad Request</h1>", "text/html; charset=utf-8", nil, {}, {}, {}, {}, {}]
    rescue ActiveRecord::RecordInvalid, ActiveRecord::RecordNotSaved, ActionController::InvalidAuthenticityToken
      # Not a 500: Rails' rescue_responses answer these with 422.
      return [422, "<h1>422 Unprocessable Content</h1>", "text/html; charset=utf-8", nil, {}, {}, {}, {}, {}]
    end

    # Dispatch on status, not on @location nil-ness: redirect_to
    # produces a 3xx status (302/303/etc.) and short-circuits to a
    # "Redirecting…" body; render-with-`location:` (Rails' POST 201
    # idiom) keeps a 2xx status and ships the rendered body alongside
    # the Location header.
    # Persist the swept flash as cookies for the next request. Flash owns
    # the show-once sweep (`to_persisted` keeps only entries this request
    # set); set those, and clear any inbound cookie that wasn't carried so
    # a displayed notice doesn't repeat.
    out_cookies = {}
    persisted = controller.flash.to_persisted
    if !host_flash.nil?
      # The host keeps the flash in its own session; hand back what
      # survives this request (nothing, once displayed).
      env["roundhouse.flash.out"] = persisted
    else
      persisted.each do |kind, value|
        name = "flash_#{kind}"
        out_cookies[name.to_sym] = ActionDispatch::SignedCookie.sign(value, name)
      end
      cookies.each_key do |cname|
        name = cname.to_s
        out_cookies[name.to_sym] = nil if name.start_with?("flash_") && !persisted.key?(name.delete_prefix("flash_"))
      end
    end
    # Cookies the action wrote (`cookies[:k] = v` / `cookies.permanent`)
    # ride out alongside the flash cookies. Flag maps stay off the
    # value so `run_rack` can emit Secure / the requested SameSite.
    jar = controller.cookies
    secure_cookies = {}
    samesite_cookies = {}
    httponly_cookies = {}
    expires_cookies = {}
    jar.pending.each do |k, v|
      out_cookies[k] = v
      secure_cookies[k] = true if jar.flag_secure?(k)
      httponly_cookies[k] = jar.flag_httponly?(k)
      ss = jar.flag_samesite(k).to_s
      samesite_cookies[k] = ss if ss.length > 0
      exp = jar.flag_expires(k)
      expires_cookies[k] = exp if exp.length > 0
    end
    # Session persistence: re-encode whatever the action (or a lazy
    # CSRF token generation during render) left in the session, and
    # Set-Cookie only on change. An emptied session (reset_session
    # logout with no token re-added) clears the cookie.
    # `request.session_options[:skip] = true` leaves it alone — see
    # the spinel main.
    session_out = controller.session.to_cookie
    if !host_session.nil?
      env["roundhouse.session.out"] = controller.session.to_hash if session_out != session_in && !controller.request.session_skip?
    elsif session_out != session_in && !controller.request.session_skip?
      out_cookies[session_cookie] =
        session_out.empty? ? nil : ActionDispatch::Session.signed_cookie(session_out, session_cookie)
    end
    is_redirect = controller.status >= 300 && controller.status < 400 && !controller.head_response?
    # Compose `Cache-Control` from whatever `response.cache_control`
    # holds (`expires_in`, `replace`/`merge!`/`[]=`) onto the buffered
    # header store, right before it is copied below — a no-op unless
    # the action or one of its filters touched the store.
    controller.commit_cache_control!
    # Rails' Rack::ETag and Rack::ConditionalGet
    # (action_controller/conditional_get.rb): the body ETag, the default
    # Cache-Control, and a 304 for a matching If-None-Match, on the
    # controller's status, body and headers before they are read below.
    controller.commit_conditional_get!(env["REQUEST_METHOD"].to_s, env["HTTP_IF_NONE_MATCH"].to_s)
    controller.commit_default_headers!
    # Headers the action set beyond Content-Type/Location — a
    # `Content-Disposition` on a download, the Cache-Control a blob
    # route asks for — ride as the tuple's sixth element.
    extra_headers = {}
    hi = 0
    hn = controller.headers.size
    while hi < hn
      extra_headers[controller.headers.key_at(hi)] = controller.headers.val_at(hi)
      hi += 1
    end
    if is_redirect
      [controller.status,
       %(<a href="#{ActionView::ViewHelpers.html_escape(controller.location.to_s)}">Redirecting</a>),
       "text/html; charset=utf-8", controller.location, out_cookies, extra_headers, secure_cookies, samesite_cookies, httponly_cookies, expires_cookies]
    else
      # The controller body IS the full page: the Ruby emit path's
      # `apply_layout_lowering` wraps each html action render in
      # `Views::Layouts.application(...)` at the render call site —
      # the only seam where the @ivars a layout reads (@user, @title)
      # are statically in scope. JSON responses ship with their own
      # Content-Type; `controller.location` (set by `render …
      # location: @article`) flows through as the Location header.
      # json and turbo_stream both carry their own Content-Type from the
      # render call site (`render …, content_type:`), so the controller's
      # value ships as-is. Turbo REQUIRES `text/vnd.turbo-stream.html`
      # here — it ignores a response typed text/html.
      # An html-format action that answered with another type
      # (`send_data … type: "image/png"` — a served avatar or logo)
      # keeps that type: the default is only the default. Same rule
      # the spinel dispatcher applies.
      if controller.request_format == :json ||
         controller.request_format == :turbo_stream ||
         controller.content_type != "text/html; charset=utf-8"
        [controller.status, controller.body,
         controller.content_type, controller.location, out_cookies, extra_headers, secure_cookies, samesite_cookies, httponly_cookies, expires_cookies]
      elsif controller.request_format == :rss
        [controller.status, controller.body,
         "application/rss+xml; charset=utf-8", controller.location, out_cookies, extra_headers, secure_cookies, samesite_cookies, httponly_cookies, expires_cookies]
      else
        [controller.status, controller.body,
         "text/html; charset=utf-8", controller.location, out_cookies, extra_headers, secure_cookies, samesite_cookies, httponly_cookies, expires_cookies]
      end
    end
  end

  # CGI entry point. Serializes the dispatch descriptor to a CGI byte
  # stream on `stdout` — the shape the one-shot script path (bottom of
  # this file) and the view/controller tests assert against. Output is
  # byte-for-byte what the prior `run` produced (same `write_response`
  # call), so those tests are unaffected by the refactor.
  def self.run(env, stdin, stdout)
    status, body, content_type, location, set_cookies, extra_headers = dispatch_core(env, stdin)
    CgiIo.write_response(stdout, status, body,
      content_type: content_type, location: location, set_cookies: set_cookies,
      extra_headers: extra_headers)
    nil
  end

  # Rack entry point — the Puma serving path (config.ru). Returns a
  # Rack response tuple `[status, headers, [body]]` directly, with NO
  # CGI string in between: the prior path serialized a CGI byte stream
  # here and re-parsed it back into this same tuple in config.ru, pure
  # round-trip overhead (~3–5µs/request). A Rack env already carries
  # CGI-style keys (REQUEST_METHOD / PATH_INFO / …) and `rack.input`,
  # so `dispatch_core` reads it without a remap. Header names are
  # lowercased per the Rack 3 convention; Set-Cookie is an Array (one
  # entry per cookie) and reuses `CgiIo.url_encode` so values match the
  # CGI path exactly.
  def self.run_rack(env)
    started = ActionDispatch::Runtime.clock_time
    status, body, content_type, location, set_cookies, extra_headers, secure_cookies, samesite_cookies, httponly_cookies, expires_cookies =
      dispatch_core(env, env["rack.input"] || StringIO.new(""))
    headers = {}
    headers["content-type"] = content_type unless content_type.empty?
    headers["location"] = location unless location.nil?
    # A nil value is a header the app unset (`X-Rev` outside a deploy
    # with GIT_REVISION) — Rack 3 refuses a nil, so it is not written.
    extra_headers.each { |k, v| headers[k.to_s.downcase] = v unless v.nil? }
    cookies = []
    https = env["HTTPS"].to_s == "on" || env.fetch("HTTP_X_FORWARDED_PROTO", "").to_s.split(",").first.to_s.strip.downcase == "https"
    set_cookies.each do |name, val|
      cookies << if val.nil?
        "#{name}=; Path=/; Max-Age=0"
      else
        ss = (samesite_cookies && samesite_cookies[name]).to_s
        ss = "Lax" if ss.empty?
        line = "#{name}=#{CgiIo.url_encode(val.to_s)}; Path=/"
        # Default HttpOnly. Session/flash cookies are not in the map.
        # An explicit httponly: false records false and is omitted.
        line = line + "; HttpOnly" unless httponly_cookies && httponly_cookies.key?(name) && !httponly_cookies[name]
        line = line + "; SameSite=#{ss}"
        line = line + "; Expires=#{expires_cookies[name]}" if expires_cookies && expires_cookies.key?(name)
        line = line + "; Secure" if https || (secure_cookies && secure_cookies[name]) || ss == "None"
        line
      end
    end
    headers["set-cookie"] = cookies unless cookies.empty?
    # Rails' ActionDispatch::RequestId and Rack::Runtime around the whole
    # request (runtime/action_dispatch/request_id.rb), as the spinel
    # dispatcher stamps them: X-Request-Id always, X-Runtime unless the
    # app set one. config.ru serves static and public files before this.
    headers["x-request-id"] = ActionDispatch::RequestId.make(env["HTTP_X_REQUEST_ID"].to_s)
    headers["x-runtime"] = ActionDispatch::Runtime.header_value(started) unless headers.key?("x-runtime")
    [status, headers, [body]]
  end

  # Maps the routes-table controller symbol to a literal `.new`
  # constructor call. Spinel's hash specializations don't accept class
  # references as values, so the route table stores symbols and this
  # case turns the symbol back into an instance via direct
  # constructor calls (statically resolvable; no `.send`).
  def self.instantiate_controller(sym)
    case sym
    when :articles then ArticlesController.new
    when :comments then CommentsController.new
    else ActiveStorage::Routes.instantiate_controller(sym)
    end
  end

  # Rails' `request.path_parameters`: the route's captures, with the
  # controller and action the request was dispatched to. The
  # controller's own `path_parameters` and params keep the captures alone.
  def self.route_parameters(controller, matched)
    out = {}
    matched.path_params.each { |k, v| out[k] = v }
    # The dispatched controller and action win over a capture of the same
    # name: Rails dispatches by these two, so they name what ran.
    out["controller"] = controller.controller_path
    out["action"] = matched.action.to_s
    out
  end

  # First-time setup. Idempotent: skips when already configured (so
  # tests that load main.rb don't conflict with their own test_helper
  # setup).
  #
  # When `BLOG_DB` env var names a path, configure SqliteAdapter
  # against that file. Otherwise default to the Rails-traditional
  # `storage/development.sqlite3` — persisted across requests and
  # consistent with every other target's default. The archive ships
  # `storage/.keep`, so the directory exists for first-run open.
  # Tests configure `:memory:` explicitly through their own setup, so
  # this server default never reaches them (the `adapter.nil?` guard
  # also short-circuits when a test already configured the adapter).
  def self.configure_default_adapter!
    return unless ActiveRecord.adapter.nil?
    db_path = ENV["BLOG_DB"]
    path = (!db_path.nil? && !db_path.empty?) ? db_path : "storage/development.sqlite3"
    # SqliteAdapter.configure delegates to Db.configure (single shared
    # connection); both the legacy AR-adapter dispatch path and the
    # Level-3 lowerer-emitted `_adapter_*` path read through one handle.
    SqliteAdapter.configure(path)
    ActiveRecord.adapter = SqliteAdapter
    Schema.statements.each { |sql| SqliteAdapter.execute_ddl(sql) }
  end
end

# Auto-run only when invoked as a script (`ruby main.rb`). When loaded
# via `require_relative "main"` from tests, the dispatch isn't
# triggered — tests call Main.run themselves with constructed I/O.
#
# The env hash is built explicitly from the CGI variables we read,
# rather than `ENV.to_h` — Spinel supports `ENV[]` indexing reliably
# but `.to_h` is on the verify list.
if __FILE__ == $PROGRAM_NAME
  Main.configure_default_adapter!
  env = {
    "REQUEST_METHOD" => ENV["REQUEST_METHOD"],
    "PATH_INFO"      => ENV["PATH_INFO"],
    "QUERY_STRING"   => ENV["QUERY_STRING"],
    "CONTENT_LENGTH" => ENV["CONTENT_LENGTH"],
    "CONTENT_TYPE"   => ENV["CONTENT_TYPE"],
    "HTTP_COOKIE"    => ENV["HTTP_COOKIE"],
    # Response-format negotiation reads this — a Turbo-driven form POST
    # asks for `text/vnd.turbo-stream.html`. The Rack path forwards the
    # whole env; this one-shot CGI path builds an allowlist, so a header
    # dispatch depends on has to be named here.
    "HTTP_ACCEPT"    => ENV["HTTP_ACCEPT"],
    "HTTP_ACCEPT_ENCODING" => ENV["HTTP_ACCEPT_ENCODING"],
    "HTTP_IF_NONE_MATCH" => ENV["HTTP_IF_NONE_MATCH"],
    "HTTP_IF_MODIFIED_SINCE" => ENV["HTTP_IF_MODIFIED_SINCE"],
    "HTTP_TURBO_FRAME" => ENV["HTTP_TURBO_FRAME"],
    # The forgery check's two headers (runtime/request_forgery_protection.rb).
    "HTTP_X_CSRF_TOKEN" => ENV["HTTP_X_CSRF_TOKEN"],
    "HTTP_ORIGIN"       => ENV["HTTP_ORIGIN"],
    "HTTP_SEC_FETCH_SITE" => ENV["HTTP_SEC_FETCH_SITE"],
    # The HTTP Token/Basic helpers' credentials (runtime/http_authentication.rb).
    "HTTP_AUTHORIZATION" => ENV["HTTP_AUTHORIZATION"],
    "HTTP_HOST"         => ENV["HTTP_HOST"],
  }
  Main.run(env, $stdin, $stdout)
end
