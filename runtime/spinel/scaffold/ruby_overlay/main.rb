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

require "stringio"
# `Time.parse`/`Time#iso8601` — backs the datetime-column accessor
# coercion the Ruby emitter synthesizes (`apply_datetime_lowering`).
require "time"
# `CGI.escape`/`CGI.parse`/`CGI.unescape_html` — app code (lobsters
# models/extras) reaches stdlib CGI directly; Rails gets it via
# ActiveSupport, the CRuby tree gets it here.
require "cgi"
# stdlib ERB for app code that escapes explicitly (`ERB::Util.html_escape`
# in lobsters' Hat#to_html_label) — the util module, not the templating.
require "erb"

# SqliteAdapter is hoisted to top-level so the spinel-AOT compile
# can statically resolve the `SqliteAdapter` constant referenced
# from `Base#save` etc. via the adapter dispatcher. Under CRuby the
# require is harmless (the gem-backed shim only opens a DB on
# `configure`); under spinel the FFI-backed shim only emits when
# `runtime/sqlite_adapter` is in the require graph.
require_relative "runtime/sqlite_adapter"
# Db primitive surface — backs the lowerer-emitted `_adapter_*`
# methods (Level-3 emit + Phase 1 Arel inline-SELECT expansions).
# Required before active_record so Base.rb's default `_adapter_*`
# helpers and per-model overrides find `Db` at constant-resolution
# time. See project_arel_compile_time_first.md.
require_relative "runtime/db"
# Base64 + JSON + Importmap shims. All required before any framework
# Ruby file that references them so spinel-AOT's static resolver
# sees the constants. The per-app config/importmap.rb (when emitted)
# reopens Importmap with the source-derived pins/entry; Base64 and
# JSON have no per-app override. Under CRuby these shims override
# the stdlib equivalents with semantically-identical implementations
# for the surface framework Ruby actually uses.
require_relative "runtime/base64"
require_relative "runtime/json"
# JsonBuilder — the JSON encoding primitives the Jbuilder lowerer
# emits calls to (`Views::Articles.article_json` etc.). Separate from
# `runtime/json.rb`'s `JSON.generate` shim: this module exposes
# `JsonBuilder.encode_value` / `encode_string` for per-value encoding.
require_relative "runtime/json_builder"
require_relative "runtime/importmap"
require_relative "runtime/rails"
# Real in-mem cache store behind Rails.cache (CRuby-only; the shared
# runtime's Cache is a recompute-every-fetch no-op).
require_relative "runtime/rails_cache"
require_relative "runtime/rails_application_routes"
require_relative "runtime/active_support_duration"
require_relative "runtime/active_support_time_parsing"
require_relative "runtime/active_support_try"
require_relative "runtime/active_support_core_ext"
require_relative "runtime/action_view_date_helper"
require_relative "runtime/action_view_number_helper"
require_relative "runtime/action_view_url_for"
require_relative "runtime/action_view_form_builder_extras"
require_relative "runtime/action_view_capture_helper"
require_relative "runtime/active_record"
require_relative "runtime/active_record_bang"
require_relative "runtime/active_record_serialization"
require_relative "runtime/active_record_relation_ext"
require_relative "runtime/active_record_arel"
require_relative "config/schema"
require_relative "runtime/action_dispatch"
require_relative "runtime/action_controller"
require_relative "runtime/action_controller_cookies"
# After action_controller: its require chain loads the shared
# action_view/view_helpers, and the safe-buffer overrides must win
# that reopen (same ordering contract as action_controller_session's
# form_authenticity_token override below).
require_relative "runtime/action_view_safe_buffer"
require_relative "runtime/action_view_missing_template"
require_relative "runtime/action_dispatch_request"
require_relative "runtime/action_controller_session"
require_relative "runtime/action_controller_json_render"
require_relative "runtime/typed_store"
require_relative "runtime/action_mailer"
# App-code gem dependencies, guarded so apps that don't use them (the
# blog) boot without the gems installed. Under Rails, Bundler
# auto-requires these; the transpiled tree loads them here so app
# classes that reach gem constants at LOAD time (lobsters'
# html_encoder.rb runs `HTMLEntities.new` in its class body) or at
# request time (bcrypt behind the synthesized User#authenticate, rotp
# behind 2FA, markly+nokogiri behind Markdowner.to_html) resolve.
["bcrypt", "htmlentities", "rotp", "markly", "nokogiri"].each do |gem_name|
  begin
    require gem_name
  rescue LoadError
    nil
  end
end
require_relative "runtime/broadcasts"
require_relative "runtime/cgi_io"
require_relative "config/routes"
# Per-app Importmap override (generated by Roundhouse from the source
# app's config/importmap.rb). Conditional because source apps without
# an importmap don't have this file emitted; the runtime/importmap.rb
# fallback stands in that case. The `begin/rescue` is CRuby-style
# error handling — spinel's static analyzer ignores the rescue path,
# which is fine here because Importmap is already defined by the
# fallback require above.
begin
  require_relative "config/importmap"
rescue LoadError
end
# Per-app Rails::Application reopen (generated from the source app's
# config/application.rb) — real config methods (`read_only?`, `name`)
# reached via `Rails.application`. Conditional like the importmap:
# source apps without one fall back to the empty runtime shim class.
begin
  require_relative "config/application"
rescue LoadError
end
require_relative "app/views"

module Main
  # Dispatch one request to a response descriptor — the single source
  # of routing / controller / flash / redirect logic. Returns the
  # 5-tuple `[status, body, content_type, location, set_cookies]` (the
  # exact argument shape `CgiIo.write_response` consumes), leaving
  # serialization to the caller. Two thin wrappers sit on top:
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
  def self.dispatch_core(env, stdin)
    # Rails wraps every request in the AR query cache: identical
    # SELECTs within one request replay the first result; any write
    # invalidates. The CRuby Db shim implements the same discipline
    # (fiber-local, so Puma threads don't share entries).
    Db.query_cache_begin
    begin
      dispatch_core_inner(env, stdin)
    ensure
      Db.query_cache_end
    end
  end

  def self.dispatch_core_inner(env, stdin)
    ActionView::ViewHelpers.reset_slots!
    Broadcasts.reset_log!

    request = CgiIo.parse_request(env, stdin)
    # Per-request format inference. Strip a `.json` suffix from the
    # request path before route matching (so `/articles/1.json` and
    # `/articles/1` share one route entry) and remember the format
    # so the controller's `respond_to`-flattened branch can pick the
    # right view + Content-Type. Default html for any unrecognized
    # extension.
    request_format = :html
    request_path = request[:path]
    if request_path.end_with?(".json")
      request_format = :json
      request_path = request_path[0...-5]
    end
    # Prepend ROOT so a GET / request matches before falling through
    # to TABLE. ROOT is kept as a separate constant in routes.rb for
    # legibility (it's the only literal-pattern entry); the dispatch
    # composes them here so Router.match stays a flat-table walk.
    matched = ActionDispatch::Router.match(request[:method], request_path,
                           [Routes.root] + Routes.table)
    if matched.nil?
      return [404, "<h1>404 Not Found</h1>", "text/html; charset=utf-8", nil, {}]
    end
    # A route-forced format (`get "/rss" => "home#index", :format =>
    # "rss"`) overrides the path-suffix sniff — the URL has no
    # extension but the route pins the response format.
    request_format = matched.req_format unless matched.req_format.nil?

    controller = Main.instantiate_controller(matched.controller)
    merged = matched.path_params.dup
    request[:params].each { |k, v| merged[k] = v }
    controller.params  = merged

    # Decode inbound flash from cookies. Each flash key carries via
    # its own cookie (`flash_notice`, `flash_alert`) so the cookie
    # plumbing stays format-free.
    cookies = request[:cookies] || {}
    # Cookie-carried session: restore the whole session from the
    # `_session` cookie (url-encoded k=v pairs; empty when absent or
    # garbled — "logged out", never a 500). The raw inbound value is
    # kept so the persist step below can skip Set-Cookie when the
    # action left the session untouched.
    session_in = cookies[:_session].to_s
    controller.session = ActionDispatch::Session.from_cookie(session_in)
    # Expose the inbound cookies to the controller as a CookieJar so
    # `cookies[:k]` reads (and `cookies[:k] = v` records writes, surfaced
    # below as Set-Cookie). CookieJar is the CRuby-only overlay class.
    controller.cookies = ActionController::CookieJar.new(cookies)
    # Load inbound flash through the constructor (NOT `flash[:k]=`) so the
    # Flash snapshots these as carried-in; `to_persisted` then sweeps the
    # ones merely displayed (show-once). See ActionDispatch::Flash.
    inbound_flash = {}
    inbound_flash["notice"] = cookies[:flash_notice] if cookies.key?(:flash_notice)
    inbound_flash["alert"]  = cookies[:flash_alert]  if cookies.key?(:flash_alert)
    controller.flash = ActionDispatch::Flash.new(inbound_flash)

    controller.request_method = request[:method]
    controller.request_path   = request[:path]
    controller.request_format = request_format
    # The full request object (CRuby overlay class) — filters read
    # `request.remote_ip` / `request.env` / `request[:format]`. `env.to_h`
    # detaches a plain mutable Hash (callers write scratch keys the real
    # ENV would reject); params delegation gets the same merged hash the
    # controller sees.
    controller.request = ActionDispatch::Request.new(env.to_h, merged)
    # Same object, module-reachable — helpers are module functions with
    # no controller context (see ActionController::Current).
    ActionController::Current.request = controller.request
    # Park the controller too: the CSRF token generator (overlay
    # form_authenticity_token) reads the live session through it.
    ActionController::Current.controller = controller

    begin
      controller.process_action(matched.action)
    rescue ActiveRecord::RecordNotFound
      return [404, "<h1>404 Not Found</h1>", "text/html; charset=utf-8", nil, {}]
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
    if persisted.key?("notice")
      out_cookies[:flash_notice] = persisted["notice"]
    elsif cookies.key?(:flash_notice)
      out_cookies[:flash_notice] = nil
    end
    if persisted.key?("alert")
      out_cookies[:flash_alert] = persisted["alert"]
    elsif cookies.key?(:flash_alert)
      out_cookies[:flash_alert] = nil
    end
    # Cookies the action wrote (`cookies[:k] = v` / `cookies.permanent`)
    # ride out alongside the flash cookies.
    controller.cookies.to_set.each { |k, v| out_cookies[k] = v }
    # Session persistence: re-encode whatever the action (or a lazy
    # CSRF token generation during render) left in the session, and
    # Set-Cookie only on change. An emptied session (reset_session
    # logout with no token re-added) clears the cookie.
    session_out = controller.session.to_cookie
    if session_out != session_in
      out_cookies[:_session] = session_out.empty? ? nil : session_out
    end
    is_redirect = controller.status >= 300 && controller.status < 400
    if is_redirect
      [controller.status,
       %(<a href="#{controller.location}">Redirecting</a>),
       "text/html; charset=utf-8", controller.location, out_cookies]
    else
      # The controller body IS the full page: the Ruby emit path's
      # `apply_layout_lowering` wraps each html action render in
      # `Views::Layouts.application(...)` at the render call site —
      # the only seam where the @ivars a layout reads (@user, @title)
      # are statically in scope. JSON responses ship with their own
      # Content-Type; `controller.location` (set by `render …
      # location: @article`) flows through as the Location header.
      if controller.request_format == :json
        [controller.status, controller.body,
         controller.content_type, controller.location, out_cookies]
      elsif controller.request_format == :rss
        [controller.status, controller.body,
         "application/rss+xml; charset=utf-8", controller.location, out_cookies]
      else
        [controller.status, controller.body,
         "text/html; charset=utf-8", controller.location, out_cookies]
      end
    end
  end

  # CGI entry point. Serializes the dispatch descriptor to a CGI byte
  # stream on `stdout` — the shape the one-shot script path (bottom of
  # this file) and the view/controller tests assert against. Output is
  # byte-for-byte what the prior `run` produced (same `write_response`
  # call), so those tests are unaffected by the refactor.
  def self.run(env, stdin, stdout)
    status, body, content_type, location, set_cookies = dispatch_core(env, stdin)
    CgiIo.write_response(stdout, status, body,
      content_type: content_type, location: location, set_cookies: set_cookies)
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
    status, body, content_type, location, set_cookies =
      dispatch_core(env, env["rack.input"] || StringIO.new(""))
    headers = { "content-type" => content_type }
    headers["location"] = location unless location.nil?
    cookies = []
    set_cookies.each do |name, val|
      cookies << if val.nil?
        "#{name}=; Path=/; Max-Age=0"
      else
        "#{name}=#{CgiIo.url_encode(val.to_s)}; Path=/; HttpOnly"
      end
    end
    headers["set-cookie"] = cookies unless cookies.empty?
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
    end
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
  }
  Main.run(env, $stdin, $stdout)
end
