# frozen_string_literal: true
module Rack
  class MiniProfiler
    class Context
      PASS = 0
      PASS_DISABLED = 1
      PROFILE = 2
      DONE = 3

      attr_accessor :mode, :settings, :start, :page, :inject_js, :app_called, :flamegraph_missing,
        :current_timer, :processing_parent

      def initialize(settings, start)
        @mode = PASS
        @settings = settings
        @start = start
        @page = nil
        @current_timer = nil
        @processing_parent = nil
        @inject_js = false
        @app_called = true
        @flamegraph_missing = false
      end
    end

    # Rack::MiniProfiler#call, split where the app is called: `request_begin` is everything before `@app.call`, `request_end` everything after.
    class RequestHook < ::RequestHooks::Hook
      STORE = FileStore.new

      ADVANCED_TOOLS_MESSAGE = "This feature is disabled by default, to enable set the enable_advanced_debugging_tools option to true in Mini Profiler config.\n"
      STACKPROF_MESSAGE = "Please install the stackprof gem and require it: add gem 'stackprof' to your Gemfile"
      BLANK_PAGE = "<!DOCTYPE html>\n<html>\n  <head>\n    <title>Rack::MiniProfiler Requests</title>\n  </head>\n  <body>\n  </body>\n</html>\n"
      ASSETS = ["includes.js", "includes.css", "vendor.js"]

      def request_begin(req, res)
        config = MiniProfiler.config
        start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
        settings = ClientSettings.new(req, STORE, start)
        MiniProfiler.deauthorize_request if config.allow_authorized?
        ctx = Context.new(settings, start)
        Thread.current[:mini_profiler_private] = ctx
        path = req.path.sub("//", "/")

        skip_it = matches_action?(req, "skip") || config.skip_paths.any? { |p| path.start_with?(p) }
        return false if skip_it
        # Snapshots (`snapshot_every_n_requests`) stay off as the gem defaults them, so a pass here never takes one.
        return false if !config.pre_authorized || (config.allow_authorized? && !settings.has_valid_cookie?)

        base = config.resources_path
        if path.start_with?(base)
          ctx.mode = Context::DONE
          ctx.app_called = false
          file_name = path.sub(base, "")
          if file_name == "results"
            serve_results(req, res)
          elsif file_name == "snapshots" || file_name == "flamegraph"
            serve_unsupported_store(req, res, file_name)
          else
            serve_file(req, res, file_name)
            settings.handle_cookie(res, req.ssl?)
          end
          return true
        end

        skip_it = matches_action?(req, "disable") || settings.disable_profiling?
        if matches_action?(req, "enable")
          skip_it = false
          config.enabled = true
        end
        if skip_it || !config.enabled
          ctx.mode = Context::PASS_DISABLED
          return false
        end
        settings.disable_profiling = ClientSettings::OFF

        if matches_action?(req, "profile-gc") || matches_action?(req, "profile-memory")
          ctx.mode = Context::DONE
          ctx.app_called = false
          text_result(res, ADVANCED_TOOLS_MESSAGE, true)
          settings.handle_cookie(res, req.ssl?)
          return true
        end

        ctx.mode = Context::PROFILE
        page = create_page(req)
        ctx.page = page
        ctx.current_timer = page.root
        ctx.inject_js = config.auto_inject && req.req_headers.fetch("x-requested-with", "") != "XMLHttpRequest"
        if matches_action?(req, "normal-backtrace")
          settings.backtrace_level = ClientSettings::BACKTRACE_DEFAULT
        elsif matches_action?(req, "no-backtrace")
          settings.backtrace_level = ClientSettings::BACKTRACE_NONE
        elsif matches_action?(req, "full-backtrace") || settings.backtrace_full?
          settings.backtrace_level = ClientSettings::BACKTRACE_FULL
        end

        # Strip the validators so a profiled page is never a 304, as the gem does.
        if config.disable_caching
          req.req_headers["if-modified-since"] = ""
          req.req_headers["if-none-match"] = ""
        end

        if matches_action?(req, "flamegraph") || matches_action?(req, "async-flamegraph") ||
           req.req_headers.fetch("referer", "").include?("pp=async-flamegraph")
          # No StackProf exists for a Spinel binary, so this is the gem's answer when the stackprof gem is missing.
          ctx.flamegraph_missing = true
          return false
        end
        if path == "/rack-mini-profiler/requests"
          ctx.app_called = false
          res.status = 200
          res.headers["Content-Type"] = "text/html"
          res.body = BLANK_PAGE
          return true
        end
        false
      end

      def request_end(req, res)
        ctx = Thread.current[:mini_profiler_private]
        return nil if ctx.nil?
        Thread.current[:mini_profiler_private] = nil
        return nil if res.upgrading_ws
        ssl = req.ssl? || (ctx.app_called && AppEnvironment.serving_assume_ssl?)
        settings = ctx.settings
        if ctx.mode == Context::DONE
          return nil
        elsif ctx.mode == Context::PASS
          settings.handle_cookie(res, ssl)
          return nil
        elsif ctx.mode == Context::PASS_DISABLED
          settings.disable_profiling = ClientSettings::ON
          settings.handle_cookie(res, ssl)
          return nil
        end

        if ctx.flamegraph_missing
          text_result(res, STACKPROF_MESSAGE, false)
          settings.handle_cookie(res, ssl)
          return nil
        end
        unless !MiniProfiler.config.allow_authorized? || MiniProfiler.authorized?
          settings.handle_cookie(res, ssl)
          return nil
        end
        # trace-exceptions is not answered: it needs TracePoint, which a Spinel binary has no equivalent of.
        if matches_action?(req, "env") || matches_action?(req, "analyze-memory")
          text_result(res, ADVANCED_TOOLS_MESSAGE, true)
          settings.handle_cookie(res, ssl)
          return nil
        end
        if matches_action?(req, "help")
          replace_response(res, 200, "text/html", help(req, settings))
          settings.handle_cookie(res, ssl)
          return nil
        end

        page = ctx.page
        return nil if page.nil?
        page.root.record_time((Process.clock_gettime(Process::CLOCK_MONOTONIC) - ctx.start) * 1000, page.children_ms(page.root))
        STORE.save(page.id, page.to_json)
        STORE.set_unviewed(page.user, page.id)
        inject_profiler(res, ctx, page) if res.status >= 200 && res.status < 300
        settings.handle_cookie(res, ssl)
        nil
      end

      def matches_action?(req, action)
        req.raw_query.include?(MiniProfiler.config.profile_parameter + "=" + action) ||
          req.req_headers.fetch("x-rack-mini-profiler", "") == action
      end

      def create_page(req)
        server_name = "localhost"
        server_port = req.ssl? ? "443" : "80"
        host = req.req_headers.fetch("host", "")
        unless host.empty?
          colon = host.rindex(":")
          bracket = host.rindex("]")
          if !colon.nil? && (bracket.nil? || colon > bracket)
            server_name = host[0, colon]
            server_port = host[colon + 1, host.length - colon - 1]
          else
            server_name = host
          end
        end
        name = req.raw_query.empty? ? req.path : req.path + "?" + req.raw_query
        TimerStruct::Page.new(MiniProfiler.generate_id, name, req.verb, req.path, server_name,
          req.verb + " http://" + server_name + ":" + server_port + name, RequestHook.user(req))
      end

      # Rack::Request#ip, the gem's default `user_provider`: behind trusted proxies, the last untrusted X-Forwarded-For address.
      def self.user(req)
        remote = req.remote_host
        return remote unless RequestHook.trusted_proxy?(remote)
        forwarded = req.req_headers.fetch("x-forwarded-for", "")
        return remote if forwarded.empty?
        addrs = forwarded.split(",").map { |a| a.strip }
        i = addrs.length - 1
        while i >= 0
          return addrs[i] unless RequestHook.trusted_proxy?(addrs[i])
          i -= 1
        end
        addrs.empty? ? remote : addrs[0]
      end

      def self.trusted_proxy?(addr)
        a = addr.downcase
        return true if a == "::1" || a == "localhost" || a.start_with?("127.") || a.start_with?("10.") ||
          a.start_with?("192.168.") || a.start_with?("unix") || a.start_with?("fc") || a.start_with?("fd")
        return false unless a.start_with?("172.")
        second = a.split(".").length > 1 ? a.split(".")[1].to_i : 0
        second >= 16 && second <= 31
      end

      def inject_profiler(res, ctx, page)
        config = MiniProfiler.config
        if config.disable_caching
          delete_header(res, "etag")
          delete_header(res, "date")
        end
        cache_control = header(res, "cache-control")
        res.headers["X-MiniProfiler-Original-Cache-Control"] = cache_control unless cache_control.nil?
        delete_header(res, "cache-control")
        res.headers["Cache-Control"] = (config.disable_caching ? "no-store, " : "") + "must-revalidate, private, max-age=0"
        res.headers["X-MiniProfiler-Ids"] = ids(page).join(",")
        content_type = header(res, "content-type")
        # Tep stamps text/html on an inline body that names no type, after this runs.
        content_type = "text/html; charset=utf-8" if content_type.nil? && res.file_path.empty? && !res.body.empty?
        return nil unless ctx.inject_js && !content_type.nil? && content_type.include?("text/html") && res.file_path.empty?
        script = profile_script(page)
        ctx.inject_js = false
        body = res.body
        index = body.rindex(/<\/body>/i)
        index = body.rindex(/<\/html>/i) if index.nil?
        return nil if index.nil?
        res.body = body[0, index] + script + body[index, body.length - index]
        nil
      end

      def ids(page)
        all = [page.id]
        STORE.get_unviewed_ids(page.user).each { |id| all << id }
        all = all.uniq
        max = MiniProfiler.config.max_traces_to_show
        if all.length > max
          all = all[0, max]
          STORE.set_all_unviewed(page.user, all)
        end
        all
      end

      # views.rb's get_profile_script over html/profile_handler.js; an empty `current_id` is the gem's `current` being nil.
      def profile_script_for(current_id, ids_value)
        config = MiniProfiler.config
        path = config.resources_path
        version = ASSET_VERSION
        "<script async nonce=\"\" type=\"text/javascript\" id=\"mini-profiler\" src=\"" + path + "includes.js?v=" + version +
          "\" data-css-url=\"" + path + "includes.css?v=" + version +
          "\" data-version=\"" + version +
          "\" data-path=\"" + path +
          "\" data-current-id=\"" + current_id +
          "\" data-ids=\"" + ids_value +
          "\" data-horizontal-position=\"" + config.horizontal_position +
          "\" data-vertical-position=\"" + config.vertical_position +
          "\" data-trivial=\"" + config.show_trivial.to_s +
          "\" data-children=\"" + config.show_children.to_s +
          "\" data-max-traces=\"" + config.max_traces_to_show.to_s +
          "\" data-controls=\"" + config.show_controls.to_s +
          "\" data-total-sql-count=\"" + config.show_total_sql_count.to_s +
          "\" data-authorized=\"true\" data-toggle-shortcut=\"" + config.toggle_shortcut +
          "\" data-start-hidden=\"" + config.start_hidden.to_s +
          "\" data-collapse-results=\"" + config.collapse_results.to_s +
          "\" data-html-container=\"" + config.html_container +
          "\" data-hidden-custom-fields=\"" + config.snapshot_hidden_custom_fields.join(",") +
          "\" data-turbo-permanent=\"" + config.enable_hotwire_turbo_drive_support.to_s + "\"></script>\n"
      end

      def profile_script(page)
        profile_script_for(page.id, ids(page).join(","))
      end

      def serve_results(req, res)
        params = RequestHook.form_fields(req.raw_query)
        form = RequestHook.form_fields(RequestHook.form_body?(req) ? req.raw_body : "")
        form.each { |k, v| params[k] = v }
        id = params.fetch("id", "")
        user = RequestHook.user(req)
        if !params.fetch("group", "").empty?
          # The FileStore the Railtie picks has no snapshots, so the gem raises NotImplementedError here: a bare 500 from the server.
          replace_response(res, 500, "", "")
          return nil
        end
        json = STORE.load(id)
        if json.empty?
          STORE.set_viewed(user, id)
          replace_response(res, 404, "", "Request not found: " + RequestHook.h(id) + " - user " + RequestHook.h(user))
          return nil
        end
        unless TimerStruct::Page.viewed?(json)
          json = TimerStruct::Page.viewed(json, TimerStruct::Client.from_form(form))
          STORE.save(id, json)
          STORE.set_viewed(user, id)
        end
        if req.req_headers.fetch("x-requested-with", "") == "XMLHttpRequest"
          replace_response(res, 200, "application/json", json)
        else
          replace_response(res, 200, "text/html", share_html(json))
        end
        nil
      end

      def share_html(json)
        name = TimerStruct::Page.read_name(json)
        duration = TimerStruct::Page.read_duration(json).round(1).to_s
        # Not raw as the gem (4.0.0) writes both: request data could close the title or the script; the browser reads the same values either way.
        "<html>\n    <head>\n        <title>" + RequestHook.h(name) + " (" + duration + " ms) - Profiling Results</title>\n" \
          "        <script type='text/javascript'> var profiler = " + RequestHook.json_escape(json) + "; </script>\n" \
          "        " + profile_script_for("", "[]") + "\n" \
          "    </head>\n    <body>\n        <div class='profiler-result-full'></div>\n    </body>\n</html>\n"
      end

      def serve_unsupported_store(req, res, file_name)
        if file_name == "snapshots"
          # Not a snapshots page: the FileStore the Railtie picks raises NotImplementedError on its listing, so the gem answers a bare 500.
          MiniProfiler.authorize_request
          replace_response(res, 500, "", "")
          return nil
        end
        params = RequestHook.form_fields(req.raw_query)
        form = RequestHook.form_fields(RequestHook.form_body?(req) ? req.raw_body : "")
        form.each { |k, v| params[k] = v }
        id = params.fetch("id", "")
        if STORE.load(id).empty?
          replace_response(res, 404, "", "Request not found: " + RequestHook.h(id) + " - user " + RequestHook.h(RequestHook.user(req)))
        else
          replace_response(res, 404, "", "No flamegraph available for " + RequestHook.h(id))
        end
        nil
      end

      # Rack::Files over the gem's html/ directory, which ships only the three files the panel loads.
      def serve_file(req, res, file_name)
        verb = req.verb
        if verb == "OPTIONS"
          replace_response(res, 200, "", "")
          res.headers["allow"] = "GET, HEAD, OPTIONS"
          return nil
        end
        unless verb == "GET" || verb == "HEAD"
          files_fail(res, 405, "Method Not Allowed")
          res.headers["allow"] = "GET, HEAD, OPTIONS"
          return nil
        end
        path_info = Tep::Url.unescape(file_name)
        unless ASSETS.include?(path_info)
          files_fail(res, 404, "File not found: " + path_info)
          return nil
        end
        disk = RESOURCES_ROOT + "/" + path_info
        replace_response(res, 200, path_info.end_with?(".css") ? "text/css" : "text/javascript", "")
        res.headers["last-modified"] = File.mtime(disk).utc.strftime("%a, %d %b %Y %H:%M:%S GMT")
        res.headers["cache-control"] = "max-age=86400"
        res.send_file(disk)
        nil
      end

      def files_fail(res, status, message)
        body = message + "\n"
        replace_response(res, status, "text/plain", body)
        res.headers["x-cascade"] = "pass"
        nil
      end

      # `text_result`: a fresh header set unless the gem passes the app's own along (`headers:`), which it does only for the stackprof message.
      def text_result(res, body, fresh)
        if fresh
          replace_response(res, 200, "text/plain; charset=utf-8", body)
        else
          delete_header(res, "content-type")
          res.headers["Content-Type"] = "text/plain; charset=utf-8"
          res.body = body
        end
        nil
      end

      # The gem returns its own `[status, headers, body]`, so nothing the app wrote, its cookies included, reaches the wire.
      def replace_response(res, status, content_type, body)
        res.status = status
        res.headers = Tep.str_hash
        res.headers["Content-Type"] = content_type unless content_type.empty?
        res.set_cookies.pop while res.set_cookies.length > 0
        res.file_path = +""
        res.body = body
        nil
      end

      def help(req, settings)
        p = MiniProfiler.config.profile_parameter
        "<!DOCTYPE html>\n<html>\n  <head>\n    <title>Rack::MiniProfiler Help</title>\n  </head>\n  <body>\n" \
          "    <pre style='line-height: 30px; font-size: 16px'>\n" \
          "      This is the help menu of the <a href='" + SOURCE_CODE_URI + "'>rack-mini-profiler</a> gem, append the following to your query string for more options:\n\n" \
          "      " + link(req, "help") + " : display this screen\n" \
          "      " + link(req, "env") + " : display the rack environment\n" \
          "      " + link(req, "skip") + " : skip mini profiler for this request\n" \
          "      " + link(req, "no-backtrace") + " " + (settings.backtrace_none? ? "(*) " : "") + ": don't collect stack traces from all the SQL executed (sticky, use " + p + "=normal-backtrace to enable)\n" \
          "      " + link(req, "normal-backtrace") + " " + (settings.backtrace_default? ? "(*) " : "") + ": collect stack traces from all the SQL executed and filter normally\n" \
          "      " + link(req, "full-backtrace") + " " + (settings.backtrace_full? ? "(*) " : "") + ": enable full backtraces for SQL executed (use " + p + "=normal-backtrace to disable)\n" \
          "      " + link(req, "disable") + " : disable profiling for this session\n" \
          "      " + link(req, "enable") + " : enable profiling for this session (if previously disabled)\n" \
          "      " + link(req, "profile-gc") + " : perform gc profiling on this request, analyzes ObjectSpace generated by request\n" \
          "      " + link(req, "profile-memory") + " : requires the memory_profiler gem, new location based report\n" \
          "      " + link(req, "flamegraph") + " : a graph representing sampled activity (requires the stackprof gem).\n" \
          "      " + link(req, "async-flamegraph") + " : store flamegraph data for this page and all its AJAX requests. Flamegraph links will be available in the mini-profiler UI (requires the stackprof gem).\n" \
          "      " + link(req, "flamegraph&flamegraph_sample_rate=1") + ": creates a flamegraph with the specified sample rate (in ms). Overrides value set in config\n" \
          "      " + link(req, "flamegraph&flamegraph_mode=cpu") + ": creates a flamegraph with the specified mode (one of cpu, wall, object, or custom). Overrides value set in config\n" \
          "      " + link(req, "flamegraph&flamegraph_ignore_gc=true") + ": ignore garbage collection frames in flamegraphs. Overrides value set in config\n" \
          "      " + link(req, "flamegraph_embed") + " : a graph representing sampled activity (requires the stackprof gem), embedded resources for use on an intranet.\n" \
          "      " + link(req, "trace-exceptions") + " : will return all the spots where your application raises exceptions\n" \
          "      " + link(req, "analyze-memory") + " : will perform basic memory analysis of heap\n\n" \
          "      All features can also be accessed by adding the X-Rack-Mini-Profiler header to the request, with any of the values above (e.g. 'X-Rack-Mini-Profiler: flamegraph')\n" \
          "    </pre>\n  </body>\n</html>\n"
      end

      def link(req, postfix)
        p = MiniProfiler.config.profile_parameter
        target = req.path + "?" + req.raw_query.sub(p + "=help", p + "=" + postfix)
        p + "=<a href='" + RequestHook.h(target) + "'>" + postfix + "</a>"
      end

      def header(res, name)
        res.headers.each do |k, v|
          return v if k.downcase == name
        end
        nil
      end

      def delete_header(res, name)
        doomed = res.headers.keys.select { |k| k.downcase == name }
        doomed.each { |k| res.headers.delete(k) }
        nil
      end

      def self.form_body?(req)
        return false unless req.verb == "POST"
        type = req.req_headers.fetch("content-type", "")
        type.empty? || type.include?("application/x-www-form-urlencoded")
      end

      def self.form_fields(text)
        out = Tep.str_hash
        text.split("&").each do |pair|
          next if pair.empty?
          eq = pair.index("=")
          if eq.nil?
            out[Tep::Url.unescape(pair)] = ""
          else
            out[Tep::Url.unescape(pair[0, eq])] = Tep::Url.unescape(pair[eq + 1, pair.length - eq - 1])
          end
        end
        out
      end

      def self.json_escape(s)
        s.gsub("<", "\\u003c").gsub(">", "\\u003e").gsub("&", "\\u0026")
      end

      def self.h(s)
        s.gsub("&", "&amp;").gsub("<", "&lt;").gsub(">", "&gt;").gsub("\"", "&quot;").gsub("'", "&#39;")
      end
    end
  end
end
