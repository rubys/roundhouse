# Roundhouse Crystal server runtime — primitive HTTP listener that
# dispatches through the transpiled framework runtime.
#
# Pipeline mirrors runtime/typescript/server.ts:
#   1. Parse HTTP request → method, path, body params
#   2. ActionDispatch::Router.match(method, path, routes_table) →
#      {controller: Symbol, action: Symbol, path_params: Hash}
#   3. Look up the controller class in @@controllers, instantiate
#   4. Set @params (ActionController::Parameters), @session, @flash
#   5. Invoke controller.process_action(action)
#   6. Format @body, @status, @location into the HTTP response
#
# Controllers extend ActionController::Base (transpiled from
# runtime/ruby/action_controller/base.rb) and inherit render /
# redirect_to / head etc. The Roundhouse:: namespace here is reserved
# for primitive concerns (HTTP, sqlite, websocket); framework concerns
# live under ActionView/ActionController/ActionDispatch/ActiveRecord
# from the transpiled runtime.

require "http/server"
require "mime"
require "uri"
require "./db"
require "./cable"

module Roundhouse
  module Server
    @@layout : Proc(String, String)? = nil
    # Per-route record shape: `{method:, pattern:, controller:, action:}`.
    # Matches the RBS record-row type that `ActionDispatch::Router.match`
    # declares for its `table` parameter
    # (`Array[{ method: String, pattern: String, controller: Symbol,
    # action: Symbol }]`). Crystal renders that record as a NamedTuple,
    # and the lowerer emits route rows via the matching shorthand
    # literal (`{method: "GET", ...}`).
    alias RouteRow = ActionDispatch::Router::Route
    @@routes : Array(RouteRow) = [] of RouteRow
    @@controllers : Hash(Symbol, ActionController::Base.class) = {} of Symbol => ActionController::Base.class
    @@session : ActionDispatch::Session = ActionDispatch::Session.new
    @@flash : ActionDispatch::Flash = ActionDispatch::Flash.new

    def self.start(
      schema_sql : String,
      routes,
      controllers : Hash(Symbol, ActionController::Base.class),
      root_route = nil,
      layout : Proc(String, String)? = nil,
      db_path : String? = nil,
      port : Int32? = nil,
    ) : Nil
      resolved_path = db_path || "db/development.sqlite3"
      resolved_port = port || (ENV["PORT"]?.try(&.to_i) || 3000)

      Roundhouse::Db.open_production_db(resolved_path, schema_sql)
      ActiveRecord.adapter = Roundhouse::SqliteAdapter.new

      @@routes = if root_route
                   [root_route] + routes
                 else
                   routes
                 end
      @@controllers = controllers
      @@layout = layout

      server = HTTP::Server.new do |context|
        dispatch(context)
      end
      address = server.bind_tcp("127.0.0.1", resolved_port)
      puts "Roundhouse Crystal server listening on http://#{address}"
      server.listen
    end

    def self.dispatch(context : HTTP::Server::Context) : Nil
      ActionView::ViewHelpers.reset_slots!
      method = context.request.method.upcase
      path = context.request.path

      if path == "/cable"
        Roundhouse::Cable.handle(context)
        return
      end

      # Static assets: serve `static/<path>` for any `/assets/*` request.
      # Mirrors Rails' Propshaft URL shape — the importmap pins and
      # `stylesheet_link_tag("tailwind")` both point at /assets/<name>.
      # `bin/rh transpile crystal` writes the actual files into
      # `static/assets/` (Tailwind compile output, turbo.min.js copy).
      # Returns 404 fall-through if the file isn't present, so route
      # matching never sees an /assets/ path that's served from disk.
      if path.starts_with?("/assets/")
        file = File.join("static", path)
        if File.file?(file)
          context.response.headers["Content-Type"] =
            MIME.from_filename?(file) || "application/octet-stream"
          File.open(file) { |io| IO.copy(io, context.response.output) }
          return
        end
      end

      # Per-request format inference. Strip a `.json` suffix from the
      # request path before route matching so `/articles/1.json` and
      # `/articles/1` share one route entry, and remember the format
      # so the controller's respond_to-flattened branch can pick the
      # right view + Content-Type. Mirrors `runtime/typescript/server.
      # ts:149-161` and the Ruby scaffold's `main.rb:82-93` — every
      # target needs this glue at its server entry point since route
      # patterns are format-agnostic.
      request_format = :html
      route_path = path
      if route_path.ends_with?(".json")
        request_format = :json
        route_path = route_path[0, route_path.size - 5]
      end

      body_params = read_form_body(context.request)
      # `_method=delete|patch|put` from Rails' hidden form field is
      # always a top-level (non-nested) key, so it survives bracket-
      # parsing untouched.
      if method == "POST"
        raw_method = body_params["_method"]?
        if raw_method.is_a?(String)
          upper = raw_method.upcase
          if upper == "PATCH" || upper == "PUT" || upper == "DELETE"
            method = upper
          end
        end
      end

      matched = ActionDispatch::Router.match(method, route_path, @@routes)
      if matched.nil?
        context.response.status_code = 404
        context.response.content_type = "text/plain"
        context.response.print "Not Found: #{method} #{path}"
        return
      end

      # `matched` is now a typed `ActionDispatch::Router::MatchResult`
      # (was a `Hash[Symbol, untyped]` requiring explicit `.as(T)` per
      # field). Per-field types are baked into the class definition;
      # no narrowing or casts needed.
      ctrl_sym = matched.controller
      action = matched.action
      path_params = matched.path_params
      ctrl_class = @@controllers[ctrl_sym]?
      if ctrl_class.nil?
        context.response.status_code = 500
        context.response.content_type = "text/plain"
        context.response.print "No controller registered: #{ctrl_sym}"
        return
      end

      # Build merged params: path + query + body. Path captures and
      # query-string entries are always String leaves; form-body keys
      # may be bracket-nested (`comment[commenter]`) and surface as
      # `Hash(String, ParamValue)` sub-trees. The slot's typed value
      # union `Roundhouse::ParamValue = String | Hash(...) | Array(...)`
      # accepts either shape; the lowered `<Resource>Params.from_raw`
      # emit narrows via `is_a?(Hash)` / `is_a?(String)` at access.
      merged = {} of String => Roundhouse::ParamValue
      path_params.each { |k, v| merged[k] = v }
      context.request.query_params.each { |k, v| merged[k] = v }
      body_params.each { |k, v| merged[k] = v }

      ctrl = ctrl_class.new
      ctrl.params = merged
      ctrl.session = @@session
      ctrl.flash = @@flash
      ctrl.request_method = method
      ctrl.request_path = path
      ctrl.request_format = request_format

      begin
        ctrl.process_action(action)
      rescue err : Exception
        STDERR.puts "handler error: #{err.message}"
        STDERR.puts err.backtrace.join("\n")
        context.response.status_code = 500
        context.response.content_type = "text/plain"
        context.response.print "Server error: #{err.message}"
        return
      end

      # Carry flash forward exactly once: post-redirect, the next
      # request reads the flash, the request after that sees fresh.
      flash_for_response = ctrl.flash || ActionDispatch::Flash.new
      @@flash = ActionDispatch::Flash.new

      status = ctrl.status || 200i64
      body = ctrl.body || ""
      location = ctrl.location

      if !location.nil? && !location.empty?
        context.response.status_code = status.to_i
        context.response.headers["Location"] = location
        @@flash = flash_for_response
        return
      end

      # JSON responses skip the html layout wrap and ship the
      # controller body verbatim with the controller-supplied
      # Content-Type. Mirrors `runtime/typescript/server.ts:236-248`
      # and the Ruby scaffold's `main.rb:157-167` branch — the
      # controller's `respond_to`-flattened body picks the JSON view
      # + content_type; the server just honors it.
      if request_format == :json
        context.response.status_code = status.to_i
        context.response.content_type =
          (ctrl.content_type.empty? ? "application/json; charset=utf-8" : ctrl.content_type)
        context.response.print body
        return
      end

      # Layout wrapping: when a layout proc is configured, pass body
      # to it (mirrors TS's `layout?: (body) => string` shape).
      response_body = if (l = @@layout)
                       ActionView::ViewHelpers.set_yield(body)
                       l.call(body)
                     else
                       body
                     end

      context.response.status_code = status.to_i
      context.response.content_type = "text/html; charset=utf-8"
      context.response.print response_body
    end

    # Parse a `application/x-www-form-urlencoded` body into a nested
    # `Hash(String, Roundhouse::ParamValue)`. Rails-shape bracket keys
    # are unwrapped:
    #
    #   `comment[commenter]=Sam` → `{"comment" => {"commenter" => "Sam"}}`
    #   `tags[]=a&tags[]=b`       → `{"tags" => ["a", "b"]}`
    #   `_method=delete`          → `{"_method" => "delete"}`
    #
    # Bare (no-bracket) keys land as String leaves. Mirrors the
    # TS server's `parseFormData` + `setNestedParam` (runtime/
    # typescript/server.ts) and Spinel's `assign_form_pair`
    # (runtime/spinel/cgi_io.rb).
    def self.read_form_body(request : HTTP::Request) : Hash(String, Roundhouse::ParamValue)
      result = {} of String => Roundhouse::ParamValue
      content_type = request.headers["Content-Type"]? || ""
      return result unless content_type.starts_with?("application/x-www-form-urlencoded")
      body_io = request.body
      return result if body_io.nil?
      raw = body_io.gets_to_end
      return result if raw.empty?
      URI::Params.parse(raw) do |k, v|
        set_nested_param(result, k, v)
      end
      result
    end


    # Insert `key=val` into the nested params map, handling Rails'
    # bracket syntax. Recognized shapes:
    #
    #   `parent[child]=v` → `out[parent] = { child => v }`
    #   `parent[]=v`      → `out[parent] = [..., v]`
    #
    # Deeper nesting (`a[b][c]`) is unsupported today — the real-blog
    # fixture only exercises one level. Future work can extend the
    # recursion if an app needs it; the ParamValue type admits it.
    private def self.set_nested_param(
      into : Hash(String, Roundhouse::ParamValue),
      key : String,
      val : String,
    ) : Nil
      open_bracket = key.index('[')
      if open_bracket.nil?
        into[key] = val
        return
      end
      close_bracket = key.index(']', open_bracket + 1)
      return if close_bracket.nil?
      parent = key[0, open_bracket]
      inner = key[(open_bracket + 1)...close_bracket]
      if inner.empty?
        # `tags[]=v` — array append.
        existing = into[parent]?
        bucket = if existing.is_a?(Array)
                   existing
                 else
                   [] of Roundhouse::ParamValue
                 end
        bucket << val.as(Roundhouse::ParamValue)
        into[parent] = bucket
      else
        # `parent[child]=v` — nested hash.
        existing = into[parent]?
        bucket = if existing.is_a?(Hash)
                   existing
                 else
                   {} of String => Roundhouse::ParamValue
                 end
        bucket[inner] = val
        into[parent] = bucket
      end
    end
  end
end
