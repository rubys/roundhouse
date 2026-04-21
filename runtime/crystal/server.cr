# Roundhouse Crystal server runtime.

require "http/server"
require "uri"
require "./http"
require "./db"
require "./view_helpers"
require "./cable"

module Roundhouse
  module Server
    @@layout : Proc(String)? = nil

    def self.start(schema_sql : String, layout : Proc(String)? = nil, db_path : String? = nil, port : Int32? = nil) : Nil
      resolved_path = db_path || "storage/development.sqlite3"
      resolved_port = port
      if resolved_port.nil?
        env_port = ENV["PORT"]?
        if env_port
          resolved_port = env_port.to_i
        else
          resolved_port = 3000
        end
      end

      Roundhouse::Db.open_production_db(resolved_path, schema_sql)
      @@layout = layout

      server = HTTP::Server.new do |context|
        dispatch(context)
      end
      address = server.bind_tcp("127.0.0.1", resolved_port)
      puts "Roundhouse Crystal server listening on http://#{address}"
      server.listen
    end

    def self.dispatch(context : HTTP::Server::Context) : Nil
      Roundhouse::ViewHelpers.reset_render_state
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

      matched = Roundhouse::Http::Router.match(method, path)
      if matched.nil?
        context.response.status_code = 404
        context.response.content_type = "text/plain"
        context.response.print "Not Found"
        return
      end

      handler = matched[0]
      path_params = matched[1]
      params = {} of String => String
      path_params.each do |k, v|
        params[k] = v
      end
      body_params.each do |k, v|
        params[k] = v
      end

      ctx = Roundhouse::Http::ActionContext.new(params)
      resp = handler.call(ctx)
      status = resp.status
      if status == 0
        status = 200
      end

      if status >= 300 && status < 400 && !resp.location.empty?
        context.response.status_code = status
        context.response.headers["Location"] = resp.location
        context.response.content_type = "text/html; charset=utf-8"
        context.response.print resp.body
        return
      end

      body = resp.body
      layout = @@layout
      if !layout.nil?
        Roundhouse::ViewHelpers.set_yield(body)
        body = layout.call
      end
      context.response.status_code = status
      context.response.content_type = "text/html; charset=utf-8"
      context.response.print body
    end

    def self.read_form_body(request : HTTP::Request) : Hash(String, String)
      result = {} of String => String
      content_type = request.headers["Content-Type"]? || ""
      if !content_type.starts_with?("application/x-www-form-urlencoded")
        return result
      end
      body_io = request.body
      if body_io.nil?
        return result
      end
      raw = body_io.gets_to_end
      if raw.empty?
        return result
      end
      URI::Params.parse(raw) do |k, v|
        result[k] = v
      end
      result
    end
  end
end
