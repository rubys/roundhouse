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
require "uri"
require "./db"
require "./cable"

module Roundhouse
  module Server
    @@layout : Proc(String, String)? = nil
    # Routes are emitted by the lowerer as Symbol-keyed Hash literals
    # (`{ :method => "GET", :pattern => "/", :controller => :app,
    # :action => :index }`). Symbol keys preserve Ruby's idiom and let
    # the transpiled router access via `route[:method]` directly. Values
    # vary — method/pattern are String, controller/action are Symbol.
    @@routes : Array(Hash(Symbol, String | Symbol)) = [] of Hash(Symbol, String | Symbol)
    @@controllers : Hash(Symbol, ActionController::Base.class) = {} of Symbol => ActionController::Base.class
    @@session : ActiveSupport::HashWithIndifferentAccess = ActiveSupport::HashWithIndifferentAccess.new
    @@flash : ActiveSupport::HashWithIndifferentAccess = ActiveSupport::HashWithIndifferentAccess.new

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

      body_params = read_form_body(context.request)
      if method == "POST" && body_params.has_key?("_method")
        upper = body_params["_method"].upcase
        if upper == "PATCH" || upper == "PUT" || upper == "DELETE"
          method = upper
        end
      end

      matched = ActionDispatch::Router.match(method, path, @@routes)
      if matched.nil?
        context.response.status_code = 404
        context.response.content_type = "text/plain"
        context.response.print "Not Found: #{method} #{path}"
        return
      end

      ctrl_sym = matched[:controller]
      action = matched[:action]
      path_params = matched[:path_params]
      ctrl_class = @@controllers[ctrl_sym]?
      if ctrl_class.nil?
        context.response.status_code = 500
        context.response.content_type = "text/plain"
        context.response.print "No controller registered: #{ctrl_sym}"
        return
      end

      # Build merged params (path + query + body), String-keyed.
      # HWIA stores String keys internally; controllers' `@params[:id]`
      # access goes through HWIA's `[]` which normalizes via `Symbol#to_s`.
      merged = {} of String => String
      path_params.each { |k, v| merged[k.to_s] = v.to_s }
      context.request.query_params.each { |k, v| merged[k] = v }
      body_params.each { |k, v| merged[k] = v }

      ctrl = ctrl_class.new
      ctrl.params = ActionController::Parameters.new(merged)
      ctrl.session = @@session
      ctrl.flash = @@flash
      ctrl.request_method = method
      ctrl.request_path = path

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
      flash_for_response = ctrl.flash || ActiveSupport::HashWithIndifferentAccess.new
      @@flash = ActiveSupport::HashWithIndifferentAccess.new

      status = ctrl.status || 200i64
      body = ctrl.body || ""
      location = ctrl.location

      if !location.nil? && !location.empty?
        context.response.status_code = status.to_i
        context.response.headers["Location"] = location
        @@flash = flash_for_response
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

    def self.read_form_body(request : HTTP::Request) : Hash(String, String)
      result = {} of String => String
      content_type = request.headers["Content-Type"]? || ""
      return result unless content_type.starts_with?("application/x-www-form-urlencoded")
      body_io = request.body
      return result if body_io.nil?
      raw = body_io.gets_to_end
      return result if raw.empty?
      URI::Params.parse(raw) do |k, v|
        result[k] = v
      end
      result
    end
  end
end
