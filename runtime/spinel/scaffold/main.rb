# frozen_string_literal: true
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
require_relative "runtime/cgi_io"

module Main
  # Spinel's class reflection needs a lexical class receiver.
  FrameworkParseError = ActionDispatch::Http::Parameters::ParseError

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

  # The controller's params, built the way Rails builds them
  # (`ActionDispatch::Http::Parameters#parameters`): the body's params,
  # the query string's merged over them, the route's path captures over
  # both — each half nested by `ParamBuilder` (runtime/param_builder.rb,
  # Rails' own rules, held to Rails' answers for 2,755 query strings).
  # nil where Rails answers 400: a malformed %-escape, invalid UTF-8, a
  # nesting too deep, a name used as both an Array and a Hash.
  #
  # This replaced a re-nesting of tep's flat String->String params that
  # knew one level of one resource and kept a repeated key's last value:
  # `user_ids[]=2&user_ids[]=3` reached the controller as `{"" => "3"}`.
  #
  # A multipart body's text fields arrive by name from its parser
  # (`Tep::Request#body_fields`), so repeated multipart fields still
  # keep their last value. JSON has a separate typed channel
  # (`Tep::Request#json_body_params`) and keeps nested objects, arrays,
  # numbers, booleans, and nulls intact, then applies Rails' default
  # deep-munge rule that removes nil array members. The urlencoded body
  # and query string are parsed from their raw bytes and keep every pair.
  # Rails' "any format": an Accept that is a bare `*/*` (parameters
  # allowed), which Rails takes as a valid header whose one format is
  # Mime::ALL. A browser's list ending in `, */*` is "browser-like" and
  # Rails reads it as html instead, so a comma rules it out.
  def self.accepts_any_format?(accept)
    return false if accept.include?(",")
    accept.split(";", 2)[0].to_s.strip == "*/*"
  end

  # The body's params alone - Rails' `request_parameters`, which
  # ParamsWrapper copies from. Return a fresh root hash because
  # `request_params` merges query and path keys into its body hash.
  # URL-encoded and multipart form data are nested by `ParamBuilder`;
  # JSON is normalized by `ParamBuilder.from_json` to keep typed values
  # and Rails' default deep-munge behavior. nil where Rails answers 400,
  # including a JSON body that does not parse (Rails' ParseError).
  def self.request_body_params(req)
    body = {}
    if req.form?
      body = ParamBuilder.from_query_string(req.raw_body)
    elsif req.multipart?
      keys = []
      values = []
      present = []
      req.body_fields.each do |k, v|
        keys.push(k)
        values.push(v)
        present.push(true)
      end
      body = ParamBuilder.build(keys, values, present)
    elsif req.json?
      body = req.json_body_malformed ? nil : ParamBuilder.from_json(req.json_body_params)
    end
    body
  end

  def self.request_params(req, path_params)
    query = ParamBuilder.from_query_string(req.raw_query)
    return nil if query.nil?
    body = Main.request_body_params(req)
    return nil if body.nil?
    out = body
    query.each { |k, v| out[k] = v }
    path_params.each { |k, v| out[k] = v }
    out
  end

  # The file parts of a multipart body (`Tep::Request#uploads`, keyed
  # `message[attachment]`), folded into the nested params after
  # `request_params` builds them, because the typed String hash it
  # iterates cannot carry a file. The resource sub-hash
  # is REBUILT with the file added rather than written into in place:
  # that is the shape spinel types as the poly-valued hash the
  # controller reads.
  def self.nest_uploads(out, uploads)
    uploads.each do |k, file|
      ob = k.index("[")
      cb = ob.nil? ? nil : k.index("]", ob + 1)
      if ob.nil? || cb.nil?
        out[k] = file
      else
        outer = k[0, ob].to_s
        inner = k[(ob + 1)...cb].to_s
        merged = {}
        existing = out[outer]
        if existing.is_a?(Hash)
          existing.each { |sk, sv| merged[sk.to_s] = sv }
        end
        merged[inner] = file
        out[outer] = merged
      end
    end
    nil
  end

  def self.nest_upload_parts(out, parts)
    parts.each do |part|
      CgiIo.assign_form_pair(out, part.name, part.file)
    end
    nil
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
  # WHERE THE DATABASE IS, decided in ONE place. `config/puma.rb` has to
  # answer the same question after every fork, and when it carried its
  # own copy of this default the two disagreed: it fell back to
  # `:memory:`, so a clustered, preloaded Puma gave every worker a fresh
  # EMPTY database and campfire's sign-in came back `no such table:
  # bans` from a file whose `bans` table is right there on disk.
  def self.default_db_path
    db_path = ENV["BLOG_DB"]
    (!db_path.nil? && !db_path.empty?) ? db_path : "storage/development.sqlite3"
  end

  def self.configure_default_adapter!
    return unless ActiveRecord.adapter.nil?
    path = Main.default_db_path
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

  # Rails' ActionDispatch::Static: a GET/HEAD for a file that exists
  # under public/ (robots.txt, the error pages) is served ahead of the
  # routes. Only a path whose last segment names a file (has an
  # extension) is looked up, so a route like `/rooms` never stats the
  # disk; the `..` guard is static_asset?'s.
  def self.public_file?(verb, path)
    return false unless verb == "GET" || verb == "HEAD"
    return false if path.include?("..")
    slash = path.rindex("/")
    return false if slash.nil?
    return false unless path[slash + 1, path.length].to_s.include?(".")
    Sock.sphttp_filesize("public" + path) >= 0
  end

  # Resolve a (pre-validated) static URL to its on-disk path under
  # static/ and hand it to the Tep server to sendfile. The server
  # doesn't infer Content-Type, so set it from the extension here; a
  # missing file 404s (Sock.sphttp_filesize returns -1 when stat fails). The
  # working directory is the app root — where `make assets` writes
  # static/ and `./build/blog` is launched from.
  def self.serve_static(path, res, root = "static")
    disk = root + path
    if Sock.sphttp_filesize(disk) < 0
      res.status = 404
      res.body = "<h1>404 Not Found</h1>"
      return nil
    end
    res.headers["Content-Type"] = Main.asset_content_type(path)
    # `config.public_file_server.headers`, as ActionDispatch::Static
    # sends them on every public/ and /assets/ file.
    Rails.application.public_file_server_headers.each do |name, value|
      res.headers[name] = value
    end
    res.send_file(disk)
    nil
  end

  # A 404 the app did not rescue (no route, ActionNotFound,
  # RecordNotFound), answered as Rails' production exceptions app
  # (ActionDispatch::PublicExceptions) does: `{"status":404,"error":
  # "Not Found"}` when the request's format is JSON
  # (ActionDispatch::MimeNegotiation.json_format?), the HTML page
  # otherwise. `format_param` is the request's `format` parameter, ""
  # when it has none.
  def self.render_not_found(res, format_param, path, req)
    res.status = 404
    extension = path.end_with?(".json") ? "json" : ""
    if ActionDispatch::MimeNegotiation.json_format?(format_param, extension,
         req.req_headers.fetch("accept", ""), req.req_headers.fetch("x-requested-with", ""),
         req.req_headers.fetch("content-type", ""))
      res.headers["Content-Type"] = "application/json; charset=UTF-8"
      res.body = "{\"status\":404,\"error\":\"Not Found\"}"
    else
      res.body = "<h1>404 Not Found</h1>"
    end
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
    elsif path.end_with?(".html")
      "text/html; charset=utf-8"
    elsif path.end_with?(".txt")
      "text/plain; charset=utf-8"
    elsif path.end_with?(".ico")
      "image/vnd.microsoft.icon"
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
    @route_table ||= [RouteTable.root] + RouteTable.table + ActiveStorage::Routes.table
  end

  # Tep::Server callback. Routes, runs the controller, copies
  # status/body/location back, and persists per-request flash via
  # cookies (one flash_<key> each) so a `redirect_to … notice:`
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
  # A queued job is a closure over its arguments or, for a job whose
  # arguments serialize, a closure over its ActiveJob payload that runs
  # it through the generated job registry (runtime/job_registry.rb).
  #
  # NOTHING IS RETRIED and nothing survives a restart. The queue is a
  # process-local Array — that is the honest shape for a single-worker
  # binary with no store, and it is the ledgered limit rather than an
  # oversight (docs/pipeline/runtime.md). What it buys over running
  # inline is that a job's latency and a job's failure both stop
  # belonging to a request.
  def self.job_loop
    while true
      sleep(JOB_POLL_INTERVAL)
      if ActiveJob.pending_count > 0
        Db.with_connection { ActiveJob.drain }
      end
    end
    0
  end

  # Rails' Rack::Runtime and ActionDispatch::RequestId around the whole
  # request (runtime/action_dispatch/request_id.rb): every response the
  # app answers carries X-Request-Id and X-Runtime, its error pages
  # included. They sit behind ActionDispatch::Static, so a file sent
  # from static/ or public/ carries neither, and the /cable upgrade is
  # left as Action Cable answers it.
  #
  # A gem's middleware (runtime/request_hooks.rb) sits outside all of it,
  # where its Railtie inserts it: at position 0.
  def self.dispatch(req, res)
    unless RequestHooks.request_begin(req, res)
      Main.dispatch_app(req, res)
    end
    RequestHooks.request_end(req, res)
    nil
  end

  def self.dispatch_app(req, res)
    started = ActionDispatch::Runtime.clock_time
    Main.dispatch_request(req, res)
    return nil if res.file_path.length > 0 || res.upgrading_ws
    res.headers[ActionDispatch::RequestId::HEADER] =
      ActionDispatch::RequestId.make(req.req_headers.fetch("x-request-id", ""))
    # Rack::Runtime leaves a value the app set itself.
    unless res.headers.keys.any? { |k| k.downcase == "x-runtime" }
      res.headers[ActionDispatch::Runtime::HEADER] = ActionDispatch::Runtime.header_value(started)
    end
    nil
  end

  # Serve static files or dispatch the complete request path through Router,
  # preserving literal suffix routes while negotiating the response format.
  def self.dispatch_request(req, res)
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
    if Main.public_file?(req.verb, req.path)
      Main.serve_static(req.path, res, "public")
      return
    end

    request_format = :html
    request_path = req.path
    if request_path.end_with?(".json")
      request_format = :json
    end
    # Router.match owns format-suffix matching and its literal-path fallback.
    # Passing the full path also lets an explicit `/feed.json` route match.
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
      # No route: the format is the `?format=` param, a trusted Accept,
      # or the path's extension (Main.render_not_found).
      Main.render_not_found(res, req.req_params.fetch("format", ""), request_path, req)
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
    request_format = :xml if path_format == "xml"
    # `/service-worker.js`: campfire's raw service-worker template.
    request_format = :js if path_format == "js"
    # A route's default format (`get "/rss" => "home#index", :format =>
    # "rss"`, `defaults: { format: :json }`) fills in what the request
    # left out, as in Rails: only when the path carried no extension, and
    # it is then written to `params[:format]` too. Without it every
    # route-pinned entry fell through to :html and lobsters' /rss and
    # /hottest served the HTML home page; with it unconditional,
    # `/widgets/1.html` on a json-defaulted route was answered as json.
    #
    # The CRuby overlay writes this generically (`request_format =
    # matched.req_format`). That shape does not compile here, and neither
    # does binding the read to a guarded local: `req_format` is
    # `Symbol?`, which spinel stores as a poly RbVal, while a local
    # seeded from a Symbol literal is a bare `sp_sym` - the assignment is
    # a type error whatever the nil guard looks like. COMPARING the poly
    # against literals never materializes a nilable Symbol, so the
    # supported formats are named here instead. The set matches the
    # response types the tail of this method can stamp.
    if path_format.empty?
      if matched.req_format == :rss
        request_format = :rss
        matched.path_params["format"] = "rss"
      elsif matched.req_format == :json
        request_format = :json
        matched.path_params["format"] = "json"
      elsif matched.req_format == :xml
        request_format = :xml
        matched.path_params["format"] = "xml"
      end
    end

    controller = Main.instantiate_controller(matched.controller)
    # The nested Rails-style params (params["article"]["title"]) the
    # per-resource *Params.from_raw factories expect; see request_params.
    params = Main.request_params(req, matched.path_params)
    if params.nil?
      res.status = 400
      res.body = "<h1>400 Bad Request</h1>"
      return
    end
    controller.params = params
    controller.path_parameters = matched.path_params
    Main.nest_upload_parts(controller.params, req.upload_parts) if req.upload_parts.length > 0
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
    request_obj.path_parameters = Main.route_parameters(controller, matched)
    # The controller's params, copied into the request's own slot (as
    # `Request.for` copies, see runtime/action_dispatch/request.rb):
    # `request.filtered_parameters` reads them.
    params.each { |k, v| request_obj.params[k] = v }
    fmt_name = "html"
    fmt_name = "json" if request_format == :json
    fmt_name = "rss" if request_format == :rss
    fmt_name = "turbo_stream" if request_format == :turbo_stream
    fmt_name = "js" if request_format == :js
    fmt_name = "xml" if request_format == :xml
    request_obj.format = fmt_name
    request_obj.body = req.raw_body
    # Write straight into the RBS-pinned `@env` (Hash[String, untyped] ->
    # StrPolyHash), which already owns the representation. Building a local
    # `env = {}` here fills it with only String values, so spinel infers it
    # StrStrHash and the `request_obj.env = env` assignment warns on the
    # StrStr->StrPoly mismatch. Writing Strings into the poly field in place
    # is a valid poly-member write and never constructs the competing hash.
    user_agent = req.req_headers.fetch("user-agent", "")
    # `request.headers` follows Rails' HTTP-to-CGI env mapping. Copy all
    # parsed headers, not just those the dispatcher itself reads: cache
    # validators, `Accept-Encoding`, and `Turbo-Frame` are consumed by
    # application code after dispatch. Write directly into the RBS-pinned
    # env field so each value keeps its poly-hash representation. This
    # includes `X-Forwarded-Proto`, CSRF/origin/fetch metadata, auth, and
    # content type; keeping one mapping avoids a drifting header allowlist.
    req.req_headers.each do |name, value|
      request_obj.env[ActionDispatch::Http::Headers.env_name_from_header(name)] = value
    end
    request_obj.env["roundhouse.listener"] = req.listener_transport
    # config.assume_ssl: TLS ends at a proxy that forwards plain http.
    # Rails' AssumeSSL marks the request https before the action, so
    # secure cookies and absolute redirects use https.
    if AppEnvironment.serving_assume_ssl?
      request_obj.env["HTTPS"] = "on"
      request_obj.env["HTTP_X_FORWARDED_PROTO"] = "https"
      request_obj.env["rack.url_scheme"] = "https"
    end
    # `user_agent` is ALSO a separate reader slot. The overlay twin's
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
    # The body's params alone, for ParamsWrapper (`Params.wrap`).
    body_params = Main.request_body_params(req)
    request_obj.request_parameters = body_params unless body_params.nil?
    # The query string's params alone (`request.query_parameters`), nested
    # by the same builder `request_params` merged them with.
    query_params = ParamBuilder.from_query_string(req.raw_query)
    request_obj.query_parameters = query_params unless query_params.nil?
    controller.request = request_obj
    controller.query_string = request_obj.query_string
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
    # SIGNED (runtime/signed_cookies.rb): a value that does not
    # verify restores as an empty session. `session_in` is the restored
    # session's PLAIN encoding — what the persist step below compares
    # against, so an untouched session writes no Set-Cookie and a
    # rejected one is not rewritten until the action changes it.
    restored = ActionDispatch::Session.from_signed_cookie(
      req.cookies.fetch(session_cookie, ""), session_cookie)
    session_in = restored.to_cookie
    controller.assign_http_session(restored)
    # Controller-level cookie access (`cookies[:k]` reads, `cookies[:k] = v`
    # records writes surfaced as Set-Cookie below). The inbound jar is the
    # request's parsed cookies (String-keyed Tep.str_hash); the CookieJar
    # normalizes keys so Symbol-constant indexing (`cookies[:tag_filters]`)
    # resolves. Same shared ActionController::CookieJar the CRuby overlay uses.
    controller.cookies = ActionController::CookieJar.new(req.cookies)
    # Inbound flash: each message rides its own SIGNED cookie
    # (flash_<key>; runtime/signed_cookies.rb) — one that
    # does not verify reads as no message, so a client cannot make the
    # next page show text of its choosing. Load
    # through the constructor (NOT flash[:k]=) so to_persisted's show-once
    # diff sees them as carried-in (value == @notice_was) and sweeps them.
    # `fetch(name, "")` avoids a missing-key raise; a flash message is
    # never empty, so non-empty == present.
    inbound_flash = Tep.str_hash
    # Every `flash_<key>` cookie, not only notice/alert: lobsters' layout shows `flash[:error]` / `[:success]`.
    req.cookies.each do |cname, cvalue|
      if cname.start_with?("flash_")
        fin = ActionDispatch::SignedCookie.verified(cvalue, cname)
        inbound_flash[cname[6, cname.length - 6]] = fin if fin.length > 0
      end
    end
    controller.flash = ActionDispatch::Flash.new(inbound_flash)
    controller.request_format = request_format
    controller.accepts_any_format = Main.accepts_any_format?(req.req_headers.fetch("accept", ""))
    # Rails' `Vary: Accept` on a render whose format came from the Accept
    # header (runtime/action_dispatch/mime_negotiation.rb). A `format`
    # param - the path extension, `?format=`, a route's own format -
    # is read instead of Accept.
    controller.vary_on_accept = ActionDispatch::MimeNegotiation.should_apply_vary_header?(
      req.req_headers.fetch("accept", ""), req.req_headers.fetch("x-requested-with", ""),
      req.req_headers.fetch("content-type", ""),
      controller.params.key?("format") || !matched.req_format.nil?)

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
      format_param = request_format == :html ? req.req_params.fetch("format", "") : fmt_name
      Main.render_not_found(res, format_param, request_path, req)
      return
    rescue ActionController::ParameterMissing, ActionController::BadRequest,
           ActionDispatch::Http::Parameters::ParseError => error
      # The app's handlers have already run. Rails' rescue_responses map
      # an unhandled ParseError to :bad_request by its exact class name;
      # an application subclass has no default mapping.
      # Spinel has no built-in method binding: compare nominal classes,
      # without consulting any application's subclass matcher.
      raise if FrameworkParseError === error && error.class != FrameworkParseError
      res.status = 400
      res.body = "<h1>400 Bad Request</h1>"
      return
    rescue ActiveRecord::RecordInvalid, ActiveRecord::RecordNotSaved, ActionController::InvalidAuthenticityToken
      # Not a 500: Rails' rescue_responses answer these with 422.
      res.status = 422
      res.body = "<h1>422 Unprocessable Content</h1>"
      return
    end

    # Compose `Cache-Control` from whatever `response.cache_control`
    # holds (`expires_in`, `replace`/`merge!`/`[]=`) onto the buffered
    # header store - a no-op unless the action or one of its filters
    # touched the store. Then Rails' Rack::ETag and Rack::ConditionalGet
    # (action_controller/conditional_get.rb): the body ETag, the default
    # Cache-Control, and a 304 for a matching If-None-Match. Both run
    # before the status, body and headers are copied below, since a 304
    # changes all three.
    controller.commit_cache_control!
    controller.commit_conditional_get!(req.verb, req.req_headers.fetch("if-none-match", ""))
    controller.commit_default_headers!

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
    if controller.content_type.empty?
      # Head responses in Rails omit Content-Type for statuses that do
      # not permit a body (1xx, 204, 205, and 304).
    elsif request_format == :rss
      res.headers["Content-Type"] = "application/rss+xml; charset=utf-8"
    elsif controller.content_type != "text/html; charset=utf-8"
      res.headers["Content-Type"] = controller.content_type
    end
    # Headers the action set beyond those two — a `Content-Disposition`
    # on a download, the Cache-Control a blob route asks for. A nil
    # value is a header the app UNSET (campfire's `X-Rev` is
    # `ENV["GIT_REVISION"]`, absent outside its own deploy) and is not
    # written: the wire has no spelling for it.
    i = 0
    n = controller.headers.size
    while i < n
      k = controller.headers.key_at(i)
      v = controller.headers.val_at(i)
      res.headers[Tep.header_name(k)] = v unless v.nil?
      i += 1
    end

    # Outbound flash: persist messages set THIS request for the NEXT one.
    # `to_persisted` returns only the entries this request set (show-once);
    # clear any inbound cookie that wasn't re-persisted so a consumed flash
    # doesn't stick on the next nav.
    persisted = controller.flash.to_persisted
    persisted.each do |kind, pv|
      if pv.length > 0
        fname = "flash_" + kind
        Main.set_flash_cookie(res, fname, ActionDispatch::SignedCookie.sign(pv, fname), request_obj.ssl?)
      end
    end
    req.cookies.each do |cname, cvalue|
      if cname.start_with?("flash_") && cvalue.length > 0 && persisted.fetch(cname[6, cname.length - 6], "").length == 0
        Main.clear_flash_cookie(res, cname)
      end
    end

    # Outbound cookies: serialize whatever the action recorded via
    # `cookies[:k] = v` / `cookies.permanent[:k] = v` as Set-Cookie. Empty
    # on a read-only request (the common GET path), so this is a no-op there.
    out_cookies = controller.cookies.pending
    ock = out_cookies.keys
    ci = 0
    jar = controller.cookies
    while ci < ock.length
      cname = ock[ci]
      copts = Tep.str_hash
      copts["Path"] = "/"
      copts["HttpOnly"] = +"" if jar.flag_httponly?(cname)
      ss = jar.flag_samesite(cname)
      copts["SameSite"] = ss if ss.length > 0
      # SameSite=None is ignored by browsers unless Secure is set.
      copts["Secure"] = +"" if jar.flag_secure?(cname) || request_obj.ssl? || ss == "None"
      # `cookies.permanent` — without it the cookie ends with the browser.
      exp = jar.flag_expires(cname)
      copts["Expires"] = exp if exp.length > 0
      res.set_cookie(cname, out_cookies[cname], copts)
      ci += 1
    end

    # Session persistence: re-encode whatever the action (or a lazy
    # CSRF token generated during the layout render above) left in the
    # session; Set-Cookie only on change. An emptied session
    # (reset_session) clears the cookie. Runs after the render so
    # lazily-created tokens are captured, and on redirects too (login
    # sets `session[:u]` then 302s).
    # `request.session_options[:skip] = true` (lobsters'
    # `clear_session_cookie`) leaves the cookie alone, as Rails'
    # session middleware skips its commit.
    session_out = controller.session.to_cookie
    if session_out != session_in && !request_obj.session_skip?
      if session_out == ""
        Main.clear_flash_cookie(res, session_cookie)
      else
        Main.set_flash_cookie(res, session_cookie,
          ActionDispatch::Session.signed_cookie(session_out, session_cookie), request_obj.ssl?)
      end
    end
  end

  # Flash cookies are HttpOnly + Path=/; the read side is server-only
  # (no JS access). A set carries the message to the next request; a
  # clear (empty value + Max-Age=0) expires a consumed one.
  def self.set_flash_cookie(res, name, value, secure)
    opts = Tep.str_hash
    opts["Path"] = "/"
    opts["HttpOnly"] = +""
    opts["SameSite"] = "Lax"
    opts["Secure"] = +"" if secure
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
# Tep::Server::Threaded expects an app object with a `dispatch(req,
# res)` instance method. Wrap Main's class-method dispatch in a thin
# instance so spinel resolves @app.dispatch through normal user-class
# dispatch instead of trying to call a module method. The threaded
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
# `./app sidekiq [options]`: the worker role instead of the server. It
# pulls jobs from Redis and runs them until TERM, and never returns (see
# runtime/sidekiq_worker.rb). No socket is served, so no in-process
# drain thread, and a Cable transport only to publish to Redis; jobs a
# job enqueues still push to Redis.
if ARGV.length > 0 && ARGV[0] == "sidekiq"
  SidekiqWorker.prepare_pool(ARGV)
  Main.configure_default_adapter!
  Db.checkpoint_in_background!
  ActiveJob.register_drain
  # A job's broadcasts go to the web processes through Redis when
  # config/cable.yml says `adapter: redis` (runtime/redis_cable.rb).
  Broadcasts.set_transport(Cable::Transport.new) if RedisCable.enabled?
  SidekiqWorker.cli(ARGV)
end
port = (ENV["PORT"] || "3000").to_i
workers = (ENV["WORKERS"] || "1").to_i
binding_arg = ""
unix_bind = ENV["UNIX_BIND"].to_s
i = 0
while i < ARGV.length
  arg = ARGV[i]
  if arg == "--help" || arg == "-h"
    puts "usage: " + $PROGRAM_NAME + " [options]"
    puts ""
    puts "  -p, --port N     listen port (default 3000; PORT env)"
    puts "  -w, --workers N  prefork N processes (default 1; WORKERS env)"
    puts "  -b, --binding IP bind to IP (BINDING env; default localhost in development, 0.0.0.0 otherwise)"
    puts "      --unix-bind URL add unix:///absolute/path?mode=600 (UNIX_BIND env; default disabled)"
    puts "  -h, --help       show this help and exit"
    puts "  sidekiq [options]  run jobs from Redis instead of serving HTTP (sidekiq --help)"
    puts ""
    puts "  OS workers per process: one per core; SPINEL_WORKERS=N caps it"
    puts "  TEP_SERVER=fiber runs the fiber-per-connection server instead"
    puts "    (HTTP only, one thread; the measurement lane, matz/spinel#4306)"
    puts "  SQLite file: BLOG_DB env (default storage/development.sqlite3)"
    exit(0)
  elsif (arg == "--port" || arg == "-p") && i + 1 < ARGV.length
    port = ARGV[i + 1].to_i
    i += 2
  elsif (arg == "--workers" || arg == "-w") && i + 1 < ARGV.length
    workers = ARGV[i + 1].to_i
    i += 2
  elsif (arg == "--binding" || arg == "-b") && i + 1 < ARGV.length
    binding_arg = ARGV[i + 1]
    i += 2
  elsif arg == "--unix-bind" && i + 1 < ARGV.length
    unix_bind = ARGV[i + 1]
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
# The listen address as `rails server` picks it: -b, else BINDING, else
# localhost in development and 0.0.0.0 in every other environment.
if binding_arg.empty?
  binding_env = ENV["BINDING"].to_s
  rails_env = ENV["RAILS_ENV"].to_s
  binding_arg = if !binding_env.empty?
    binding_env
  elsif rails_env.empty? || rails_env == "development"
    "localhost"
  else
    "0.0.0.0"
  end
end
# An address-specific listener is not shared across prefork processes
# (SO_REUSEPORT is the every-interface listener's), so -w N > 1 needs it.
if workers > 1 && binding_arg != "0.0.0.0"
  $stderr.puts "--workers " + workers.to_s + " needs --binding 0.0.0.0 (one listener per process shares only that address)"
  exit(1)
end
Tep::APP.bind_host = binding_arg
Sock.sphttp_install_term_handlers
unix_listener = Tep::UnixListener.new(unix_bind)
unix_listener.open
begin
Main.configure_default_adapter!
# Serving, so WAL checkpoints move off the request path: a background
# thread runs them instead of some request's COMMIT
# (Db.checkpoint_in_background!).
Db.checkpoint_in_background!
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
# Under queue_adapter :sidekiq jobs go to Redis instead; say where.
SidekiqPush.announce if JobRegistry.sidekiq?
Thread.new do
  Main.job_loop
end
# The server announces itself by the app's own name (its GlobalID app
# name, which ingest writes into config/application.rb), not by the
# binary's file name.
Tep::APP.name = Rails.application.global_id_app
# Tep::Server::Threaded is the thread-per-connection server: a green
# thread per connection, every wait a park (recv, write, sleep, and the
# timed idle waits), so a held-open WebSocket costs a small stack and
# the OS workers run other connections' Ruby meanwhile. It replaced the
# fiber-per-connection server, whose poll loop sat above a blocking
# write in C that no Ruby scheduler could see.
#
# TEP_SERVER=fiber runs the server it replaced, Tep::Server::Scheduled,
# from the same binary: the two designs measured back to back with only
# the environment differing (matz/spinel#4306). HTTP only; see the note
# at the top of tep/server_scheduled.rb.
if (ENV["TEP_SERVER"] || "thread") == "fiber"
  Tep::Server::Scheduled.new(MainApp.new).run(port, workers, false, unix_listener)
else
  Tep::Server::Threaded.new(MainApp.new).run(port, workers, false, unix_listener)
end
ensure
  unix_listener.close
end
