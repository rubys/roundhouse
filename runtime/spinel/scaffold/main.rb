# Top-level entry point for the spinel-blog application.
#
# Reads a CGI request from ENV + $stdin, dispatches through the router
# + controller, writes the CGI response to $stdout. The shape spinel
# can ingest (no sockets, just env-vars + stdin + stdout) and the
# shape any CGI-aware web server can drive.
#
# Library usage (from tests):
#   require_relative "main"
#   Main.run(env_hash, body_io, response_io)
#
# Script usage:
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

  # Re-nest tep's flat bracket-keyed params into the Rails-style nested
  # hash the per-resource *Params.from_raw factories expect, and merge
  # the route's path captures (id, ...).
  #
  # tep parses a form body into flat keys (`article[title]` -> "..."`).
  # The blog nests exactly one resource per request, so collect its
  # bracketed fields into a single String->String sub-hash, then assign
  # that sub-hash into the outer poly hash alongside the bare-key
  # scalars + path captures. The "assign a String or a whole sub-hash"
  # shape (rather than mutating `out[outer][inner]` in place) is what
  # makes spinel type `out` as the String->(String|Hash) the controller
  # reads — same discipline as test_helper's stringify_keys.
  def self.nest_params(flat, path_params)
    # Split the flat bracket-keyed params into String->String locals
    # FIRST (sub = the one nested resource's fields, scalars = bare
    # keys). Iterating `flat` only into String-typed str_hashes keeps
    # `flat` (req.req_params) itself String->String — assigning its values
    # straight into the poly `out` below would back-propagate and widen
    # req.req_params to poly, breaking unrelated String reads of it.
    sub = Tep.str_hash
    scalars = Tep.str_hash
    outer_name = +""
    flat.each do |k, v|
      ob = k.index("[")
      if ob.nil?
        scalars[k] = v
      else
        cb = k.index("]", ob + 1)
        if cb.nil?
          scalars[k] = v
        else
          outer_name = k[0, ob]
          sub[k[(ob + 1)...cb]] = v
        end
      end
    end
    # Assemble the poly result. deep_dup yields Hash[String, untyped],
    # wide enough to hold both String scalars and the nested sub-hash;
    # poly-into-poly unifies (a raw StrStrHash value would not).
    out = Main.deep_dup(path_params)
    scalars.each { |k, v| out[k] = v }
    if outer_name.length > 0
      out[outer_name] = Main.deep_dup(sub)
    end
    out
  end

  # Rebuild a string-keyed Hash, recursing into nested Hash values.
  # Strictly typed `(Hash) -> Hash`; the return is Hash[String, untyped]
  # because `out` is assigned both a Hash (deep_dup(v)) and a leaf (v).
  # Identical shape to test_helper's stringify_keys — the construction
  # spinel reliably types as a poly-valued hash.
  def self.deep_dup(h)
    out = {}
    h.each do |k, v|
      if v.is_a?(Hash)
        out[k.to_s] = Main.deep_dup(v)
      else
        out[k.to_s] = v
      end
    end
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

  # True for request paths that map to a file under static/: the
  # importmap-pinned assets at /assets/* plus the layout's root icons.
  # The `..` guard blocks path-traversal escapes (`/assets/../../etc`)
  # before the path is concatenated onto the static/ root.
  def self.static_asset?(path)
    if path.include?("..")
      return false
    end
    path.start_with?("/assets/") || path == "/icon.png" || path == "/icon.svg"
  end

  # Resolve a (pre-validated) static URL to its on-disk path under
  # static/ and hand it to the Tep server to sendfile. sphttp can't
  # infer Content-Type, so set it from the extension here; a missing
  # file 404s (Sock.sphttp_filesize returns -1 when stat fails). The
  # working directory is the app root — where `make assets` writes
  # static/ and `./build/blog` is launched from.
  def self.serve_static(path, res)
    disk = "static" + path
    if Sock.sphttp_filesize(disk) < 0
      res.status = 404
      res.body = "<h1>404 Not Found</h1>"
      return nil
    end
    res.headers["Content-Type"] = Main.asset_content_type(path)
    res.send_file(disk)
    nil
  end

  # Minimal filename-extension → MIME map for the asset kinds this app
  # serves. Anything unrecognized falls back to octet-stream.
  def self.asset_content_type(path)
    if path.end_with?(".css")
      "text/css; charset=utf-8"
    elsif path.end_with?(".js")
      "text/javascript; charset=utf-8"
    elsif path.end_with?(".svg")
      "image/svg+xml"
    elsif path.end_with?(".png")
      "image/png"
    elsif path.end_with?(".json")
      "application/json"
    else
      "application/octet-stream"
    end
  end

  # The composed dispatch table, built once — the sibling of
  # ruby_overlay/main.rb's `route_table`, which this file went without.
  #
  # `RouteTable.table` is an emitted def that CONSTRUCTS every Route
  # object on each call, so calling it per request rebuilt ~200 of them
  # plus the concatenated array. On the CRuby lane that was filed as
  # allocation hygiene rather than a measured win. On the AOT lane it is
  # measured: a `sample` of a /u-only replay put `sp_PolyArray_scan` — the
  # GC marking that freshly-built route array — among the largest costs in
  # the profile, because /u allocates enough per request to trigger several
  # collections and every one of them re-marked the table.
  def self.route_table
    @route_table ||= [RouteTable.root] + RouteTable.table
  end

  # Tep::Server callback. Routes, runs the controller, copies
  # status/body/location back, and persists per-request flash via
  # cookies (flash_notice / flash_alert) so a `redirect_to … notice:`
  # shows once on the redirect target and is then swept — matching the
  # cookie-backed Rack path in ruby_overlay/main.rb. Session is still a
  # fresh per-request object (not yet cookie-persisted), same as Rack.
  # How often the job fiber looks for work, in seconds. A cooperative
  # poll rather than a wakeup because there is nothing to wake on: the
  # queue is a plain Array and the enqueue happens on another fiber
  # inside the same worker. 50 ms is below the threshold at which a
  # notification feels tied to the request that caused it, and 20
  # wakeups a second on an idle server costs a `length` check each.
  JOB_POLL_INTERVAL = 0.05

  # Drain background jobs on the scheduler, forever.
  #
  # THIS IS WHAT `perform_later` HAS ALWAYS PROMISED. Before it, the
  # binary ran jobs at the call site, so campfire's `after_create_commit
  # -> { room.receive(self) }` reached web-push delivery INSIDE the POST
  # that created the message — and on this runtime, where an unhandled
  # error ends the process rather than the request, a job that cannot
  # work takes the server with it.
  #
  # ITS OWN DB LEASE, taken per drain rather than held. `Main.dispatch`
  # wraps each request in `Db.with_connection`, and this fiber is not
  # inside one — a job that touches a model on a borrowed handle would
  # be using the connection of whatever request happened to be running.
  # Taken around the whole drain rather than per job: the jobs in one
  # pass are the ones a single request enqueued, and they are far more
  # likely to touch the same rows than not.
  #
  # NOTHING IS RETRIED and nothing survives a restart. The queue is a
  # process-local Array — that is the honest shape for a single-worker
  # binary with no store, and it is the ledgered limit rather than an
  # oversight (docs/pipeline/runtime.md). What it buys over running
  # inline is that a job's latency and a job's failure both stop
  # belonging to a request.
  def self.job_loop
    while true
      Tep::Scheduler.pause(JOB_POLL_INTERVAL)
      if ActiveJob.pending_count > 0
        Db.with_connection { ActiveJob.drain }
      end
    end
    0
  end

  def self.dispatch(req, res)
    ActionView::ViewHelpers.reset_slots!
    Broadcasts.reset_log!
    # Under RH_SQL_TRACE the request line brackets its queries, so a
    # tally script can attribute each SQL line to the request that ran it.
    if Db.sql_trace?
      $stderr.puts "-- " + req.verb + " " + req.path
    end

    # Action Cable: upgrade /cable to a WebSocket and hand off to the
    # Cable glue. Tep::Server::Scheduled's write path runs the recv
    # loop once res.start_websocket is set; none of the HTTP routing
    # below applies to the upgraded connection.
    if req.path == "/cable"
      Cable.upgrade(req, res)
      return
    end

    # Static assets: the importmap-pinned JS + the stylesheets, laid
    # out by `make assets` under static/assets/, plus the layout's root
    # icons. The Tep server sphttp-sendfiles whatever res.send_file
    # names; resolve the URL to its on-disk path + Content-Type here.
    # None of the dynamic routing below applies to a served file.
    if Main.static_asset?(req.path)
      Main.serve_static(req.path, res)
      return
    end

    request_format = :html
    request_path = req.path
    if request_path.end_with?(".json")
      request_format = :json
      request_path = request_path[0...-5]
    end
    # Turbo Stream is negotiated by the Accept header, not by a path
    # suffix — a Turbo-driven form POST asks for
    # `text/vnd.turbo-stream.html`. Checked after the suffix so an
    # explicit `.json` still wins.
    if request_format == :html &&
       req.req_headers.fetch("accept", "").include?("text/vnd.turbo-stream.html")
      request_format = :turbo_stream
    end

    # Rails-style method override: destroy/update forms POST a hidden
    # `_method=delete|patch|put` (browsers can't emit those verbs from a
    # form). Honor it before route matching so the right action runs.
    verb = req.verb
    if verb == "POST"
      override = req.req_params.fetch("_method", "").upcase
      if override == "PATCH" || override == "PUT" || override == "DELETE"
        verb = override
      end
    end

    matched = ActionDispatch::Router.match(verb, request_path, Main.route_table)
    if matched.nil?
      res.status = 404
      res.body = "<h1>404 Not Found</h1>"
      return
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
    # A route-forced format (`get "/rss" => "home#index", :format => "rss"`)
    # overrides the path-suffix sniff above — the URL carries no extension
    # but the route pins the response format. Without it every route-pinned
    # entry fell through to :html and lobsters' /rss and /hottest served the
    # HTML home page.
    #
    # The CRuby overlay writes this as a one-line `request_format =
    # matched.req_format unless matched.req_format.nil?`. That shape does
    # not compile here, and neither does binding the read to a guarded
    # local: `req_format` is `Symbol?`, which spinel stores as a poly
    # RbVal, while a local seeded from a Symbol literal is a bare `sp_sym`
    # — the assignment is a type error whatever the nil guard looks like.
    # COMPARING the poly against literals never materializes a nilable
    # Symbol, so the supported formats are named here instead. The set
    # matches the response types the tail of this method can stamp.
    if matched.req_format == :rss
      request_format = :rss
    elsif matched.req_format == :json
      request_format = :json
    end

    controller = Main.instantiate_controller(matched.controller)
    # Build the nested Rails-style params (params["article"]["title"])
    # that the per-resource *Params.from_raw factories expect. tep parses
    # the form body into flat bracket keys (req.req_params["article[title]"]);
    # re-nest them + merge the route's path captures (id, ...).
    controller.params = Main.nest_params(req.req_params, matched.path_params)
    # Typed request object + per-request context statics. Helpers are
    # module functions with no controller in scope; the emit rewrites
    # their bare `request` reads to `ActionController::Current.request`
    # and the layout reaches flash through `Current.controller`.
    request_obj = ActionDispatch::Request.new
    request_obj.request_method = verb
    request_obj.path = request_path
    qm = req.raw_path.index("?")
    request_obj.query_string = req.raw_path[qm + 1, req.raw_path.length].to_s unless qm.nil?
    request_obj.remote_ip = req.remote_host
    request_obj.referer = req.req_headers.fetch("referer", "")
    request_obj.host = req.req_headers.fetch("host", "localhost")
    fmt_name = "html"
    fmt_name = "json" if request_format == :json
    fmt_name = "rss" if request_format == :rss
    fmt_name = "turbo_stream" if request_format == :turbo_stream
    request_obj.format = fmt_name
    request_obj.body = req.raw_body
    # Write straight into the RBS-pinned `@env` (Hash[String, untyped] ->
    # StrPolyHash), which already owns the representation. Building a local
    # `env = {}` here fills it with only String values, so spinel infers it
    # StrStrHash and the `request_obj.env = env` assignment warns on the
    # StrStr->StrPoly mismatch. Writing Strings into the poly field in place
    # is a valid poly-member write and never constructs the competing hash.
    user_agent = req.req_headers.fetch("user-agent", "")
    request_obj.env["HTTP_USER_AGENT"] = user_agent
    # …AND the reader, which is a DIFFERENT slot. The overlay twin's
    # `user_agent` reads `@env["HTTP_USER_AGENT"]`; the shared runtime's
    # (`runtime/ruby/action_dispatch/request.rb`) returns `@user_agent`,
    # which only `Request.for` assigns and this path does not go
    # through. Writing the env alone left `request.user_agent` as the
    # `+""` it was initialized to, so campfire's
    # `ApplicationPlatform.new(request.user_agent)` parsed the EMPTY
    # string — which `UserAgent.parse` turns into the gem's
    # `DEFAULT_USER_AGENT`, and every room page rendered its PWA
    # instructions for "Mozilla" on an unknown OS whatever browser
    # asked. A 200 the whole time, which is why only reading the page
    # found it.
    request_obj.user_agent = user_agent
    request_obj.env["HTTP_X_REQUESTED_WITH"] = req.req_headers.fetch("x-requested-with", "")
    controller.request = request_obj
    ActionController::Current.request = request_obj
    ActionController::Current.controller = controller
    # Cookie-carried session: restore the whole session from the session
    # cookie (url-encoded k=v pairs; empty when absent or garbled).
    # Persisted below only when the encoding changed — an action that
    # leaves the session untouched emits no Set-Cookie.
    #
    # The NAME comes from `Rails.application.session_cookie_key`, not a
    # literal: apps declare it via `config.session_store :cookie_store,
    # key: "..."`, which ingest lifts onto the Rails::Application reopen
    # (framework default "_session" when they don't). App code reads the
    # same accessor, and the two must agree — lobsters'
    # `remove_unknown_cookies` deletes every cookie whose key isn't the
    # configured one, so a literal here would clear the session on every
    # request.
    session_cookie = Rails.application.session_cookie_key
    session_in = req.cookies.fetch(session_cookie, "")
    controller.assign_http_session(ActionDispatch::Session.from_cookie(session_in))
    # Controller-level cookie access (`cookies[:k]` reads, `cookies[:k] = v`
    # records writes surfaced as Set-Cookie below). The inbound jar is the
    # request's parsed cookies (String-keyed Tep.str_hash); the CookieJar
    # normalizes keys so Symbol-constant indexing (`cookies[:tag_filters]`)
    # resolves. Same shared ActionController::CookieJar the CRuby overlay uses.
    controller.cookies = ActionController::CookieJar.new(req.cookies)
    # Inbound flash: each message rides its own cookie (flash_notice /
    # flash_alert) so the value carries verbatim, no serialization. Load
    # through the constructor (NOT flash[:k]=) so to_persisted's show-once
    # diff sees them as carried-in (value == @notice_was) and sweeps them.
    # `fetch(name, "")` avoids a missing-key raise; a flash message is
    # never empty, so non-empty == present.
    inbound_flash = Tep.str_hash
    cin = req.cookies.fetch("flash_notice", "")
    inbound_flash["notice"] = cin if cin.length > 0
    ain = req.cookies.fetch("flash_alert", "")
    inbound_flash["alert"] = ain if ain.length > 0
    controller.flash = ActionDispatch::Flash.new(inbound_flash)
    controller.request_format = request_format

    begin
      controller.process_action(matched.action)
    rescue ActiveRecord::RecordNotFound
      res.status = 404
      res.body = "<h1>404 Not Found</h1>"
      return
    end

    res.status = controller.status
    # The layout wrap happens at the controller's render call sites
    # (apply_layout_lowering — the seam where the @ivars a layout reads
    # are in scope), so the body ships verbatim here. Same contract as
    # the CRuby overlay dispatch.
    res.body = controller.body
    res.headers["Location"] = controller.location unless controller.location.nil?
    # Content-Type: any render that named its own type carries it to
    # the wire — jbuilder's application/json, turbo's
    # text/vnd.turbo-stream.html (which turbo REQUIRES; it ignores a
    # response typed text/html), an avatar's image/svg+xml. The html
    # default stays a no-op here because tep stamps
    # `text/html; charset=utf-8` on any inline body that names no type
    # (server_scheduled.rb). This used to forward only for the :json
    # and :turbo_stream request formats, which left campfire's
    # initials-avatar SVG — `render …, content_type: "image/svg+xml"`
    # under an extensionless URL — served as text/html, and browsers do
    # NOT sniff SVG in an <img>: a 200 the page showed as a broken
    # image. The CRuby overlay's dispatch has always forwarded the
    # controller's type unconditionally (ruby_overlay/main.rb); this is
    # the same contract. RSS keeps its fixed feed type, matching what
    # that overlay dispatch returns for the same routes.
    if request_format == :rss
      res.headers["Content-Type"] = "application/rss+xml; charset=utf-8"
    elsif controller.content_type != "text/html; charset=utf-8"
      res.headers["Content-Type"] = controller.content_type
    end

    # Outbound flash: persist messages set THIS request for the NEXT one.
    # `to_persisted` returns only notice/alert that differ from the
    # carried-in value (show-once); clear any inbound cookie that wasn't
    # re-persisted so a consumed flash doesn't stick on the next nav.
    persisted = controller.flash.to_persisted
    pn = persisted.fetch("notice", "")
    if pn.length > 0
      Main.set_flash_cookie(res, "flash_notice", pn)
    elsif req.cookies.fetch("flash_notice", "").length > 0
      Main.clear_flash_cookie(res, "flash_notice")
    end
    pa = persisted.fetch("alert", "")
    if pa.length > 0
      Main.set_flash_cookie(res, "flash_alert", pa)
    elsif req.cookies.fetch("flash_alert", "").length > 0
      Main.clear_flash_cookie(res, "flash_alert")
    end

    # Outbound cookies: serialize whatever the action recorded via
    # `cookies[:k] = v` / `cookies.permanent[:k] = v` as Set-Cookie. Empty
    # on a read-only request (the common GET path), so this is a no-op there.
    out_cookies = controller.cookies.pending
    ock = out_cookies.keys
    ci = 0
    while ci < ock.length
      cname = ock[ci]
      copts = Tep.str_hash
      copts["Path"] = "/"
      res.set_cookie(cname, out_cookies[cname], copts)
      ci += 1
    end

    # Session persistence: re-encode whatever the action (or a lazy
    # CSRF token generated during the layout render above) left in the
    # session; Set-Cookie only on change. An emptied session
    # (reset_session) clears the cookie. Runs after the render so
    # lazily-created tokens are captured, and on redirects too (login
    # sets `session[:u]` then 302s).
    session_out = controller.session.to_cookie
    if session_out != session_in
      if session_out == ""
        Main.clear_flash_cookie(res, session_cookie)
      else
        Main.set_flash_cookie(res, session_cookie, session_out)
      end
    end
  end

  # Flash cookies are HttpOnly + Path=/; the read side is server-only
  # (no JS access). A set carries the message to the next request; a
  # clear (empty value + Max-Age=0) expires a consumed one.
  def self.set_flash_cookie(res, name, value)
    opts = Tep.str_hash
    opts["Path"] = "/"
    opts["HttpOnly"] = +""
    res.set_cookie(name, value, opts)
  end

  def self.clear_flash_cookie(res, name)
    opts = Tep.str_hash
    opts["Path"] = "/"
    opts["Max-Age"] = "0"
    opts["HttpOnly"] = +""
    res.set_cookie(name, "", opts)
  end
end

# Auto-run only when invoked as a script (`ruby main.rb`). When loaded
# via `require_relative "main"` from tests, the dispatch isn't
# triggered — tests call Main.run themselves with constructed I/O.
#
# Spinel-AOT entry: configure DB, register the Action Cable transport,
# then start the scheduled Tep server with Main as the app. This file
# is the spinel-target main.rb — only consumed by `make build` →
# compiled native binary. The CRuby development path uses
# `ruby_overlay/main.rb` (Puma/Rack), which keeps the
# `__FILE__ == $PROGRAM_NAME` guard because under Puma the file is
# required, not invoked directly.
#
# Tep::Server::Scheduled expects an app object with a `dispatch(req,
# res)` instance method. Wrap Main's class-method dispatch in a thin
# instance so spinel resolves @app.dispatch through normal user-class
# dispatch instead of trying to call a module method. The scheduled
# server's cmeth handlers actually dispatch via Tep::APP.dispatch, so
# this delegates THERE: that is where the per-request connection lease
# (and with it the query cache and the statement-cache trim) lives, and
# a lease here as well would take two connections for one request.
class MainApp
  def dispatch(req, res)
    Tep::APP.dispatch(req, res)
  end
end

# Unconditional entry — the spinel-compiled binary's sole purpose is
# to start the server. `if __FILE__ == $PROGRAM_NAME` would have
# gated this in CRuby script mode, but under spinel-AOT `__FILE__`
# is the source file name (`"main.rb"`) and `$PROGRAM_NAME` is the
# binary's argv[0] (e.g. `"./build/blog"`), so the guard always
# returns false. The Ruby-overlay sibling keeps the guard for its
# Puma/Rack-required-from-config.ru shape.
#
# CLI surface. Flags override the PORT/WORKERS env vars; --help exits
# before any DB or server setup runs. Parsed with a plain while loop
# (no OptionParser — stdlib optparse isn't in the spinel subset).
port = (ENV["PORT"] || "3000").to_i
workers = (ENV["WORKERS"] || "1").to_i
i = 0
while i < ARGV.length
  arg = ARGV[i]
  if arg == "--help" || arg == "-h"
    puts "usage: " + $PROGRAM_NAME + " [options]"
    puts ""
    puts "  -p, --port N     listen port (default 3000; PORT env)"
    puts "  -w, --workers N  prefork workers (default 1; WORKERS env)"
    puts "  -h, --help       show this help and exit"
    puts ""
    puts "  SQLite file: BLOG_DB env (default storage/development.sqlite3)"
    exit(0)
  elsif (arg == "--port" || arg == "-p") && i + 1 < ARGV.length
    port = ARGV[i + 1].to_i
    i += 2
  elsif (arg == "--workers" || arg == "-w") && i + 1 < ARGV.length
    workers = ARGV[i + 1].to_i
    i += 2
  else
    $stderr.puts "unknown option: " + arg + " (try --help)"
    exit(1)
  end
end
if port <= 0 || port > 65535
  $stderr.puts "invalid port: " + port.to_s
  exit(1)
end
Main.configure_default_adapter!
# Wire model after-commit Turbo Stream broadcasts to the live WebSocket
# fan-out. Without this, broadcasts only land in the in-memory log.
Broadcasts.set_transport(Cable::Transport.new)
# Background jobs go on the scheduler instead of running at the call
# site. `register_drain` is what flips `perform_later` from dispatching
# to enqueueing (see `lower::job_class_side`): a job is only handed to a
# queue once something has said it will drain it, so a tree that never
# reaches this line keeps the inline behaviour rather than dropping work
# into an Array nobody reads.
ActiveJob.register_drain
Tep::Scheduler.spawn_fiber(Fiber.new { Main.job_loop })
# Tep::Server::Scheduled is the fiber-per-connection server (Falcon-
# shape). Required for WebSockets: the /cable recv loop parks on
# Tep::Scheduler.io_wait so a held-open connection doesn't pin the
# worker. It serves plain HTTP too — a superset of the blocking
# Tep::Server the binary used before cable landed.
Tep::Server::Scheduled.new(MainApp.new).run(port, workers, false)
