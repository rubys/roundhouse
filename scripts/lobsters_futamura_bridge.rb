# scripts/lobsters_futamura_bridge.rb — the whole-route prototype behind
# `scripts/lobsters-rails-replay --lowered TREE`.
#
# One process, two apps. Rails boots as usual. The roundhouse
# `--target ruby` emit of the same app (TREE) is loaded into a Ruby::Box,
# so its runtime's ActiveRecord / ActionController / ActiveSupport are
# separate constants from Rails' own and the two coexist. A Rack
# middleware sitting just after Rails' session store hands every GET to
# the lowered app's dispatcher; everything else (the login POST
# included) stays with Rails.
#
# What crosses the boundary is the session, nothing else: no model
# object built on one side is ever handed to the other. Rails' decoded
# session goes in as the lowered app's own signed session cookie, and
# any key the lowered app changes is written back into Rails' session,
# which Rails' CookieStore then persists as usual. CSRF tokens are
# Rails' format on both sides (`_csrf_token`, masked with a one-time
# pad), so a form rendered by the lowered app posts to Rails and
# verifies.
#
# A prototype for measurement, not the futamura target:
#   * the lowered app reads its own in-memory copy of the database,
#     seeded the same way as Rails' copy; the benchmark is GET-only, so
#     the copies cannot diverge during a run;
#   * routing is "every GET", with a fallback to Rails when the lowered
#     app answers 404 or >= 500 (the request then runs twice; the stats
#     printed at exit count these);
#   * Ruby::Box needs RUBY_BOX=1 at process start, and Ruby 4.0.x
#     segfaults under it as soon as Rails loads net/imap
#     (attr_type_coercion.rb, Ractor.make_shareable); Ruby master works.
#     RUBY_BOX=1 also slows Rails itself (~20% on this benchmark), which
#     is why the Rails lane runs without it.
require "cgi"

module LobstersFutamuraBridge
  SESSION_KEY = "lobster_trap"
  DB_URI = "file:lobsters_lowered?mode=memory&cache=shared"

  @box = nil
  @stats = Hash.new(0)

  class << self
    attr_reader :box, :stats

    # Abort unless this process can host a box at all. Called before
    # Rails boots, so a wrong interpreter fails fast.
    def check!
      return if defined?(Ruby::Box) && Ruby::Box.respond_to?(:enabled?) && Ruby::Box.enabled?
      abort "lobsters-rails-replay --lowered needs Ruby::Box: run with RUBY_BOX=1 on Ruby master " \
            "(Ruby 4.0.x segfaults under RUBY_BOX=1 when Rails loads net/imap)"
    end

    # Load TREE's main.rb into a new box and seed its database from
    # DB_FILE with the same backup + ANALYZE the Rails side gets.
    def boot!(tree, db_file)
      ENV["BLOG_DB"] = DB_URI
      ENV["REMOTE_ADDR"] ||= "127.0.0.1"
      box = Ruby::Box.new
      box.eval(<<~RUBY)
        require #{File.join(tree, "main").inspect}
        Main.configure_default_adapter!
        require "sqlite3"
        mem  = SQLite3::Database.new(#{DB_URI.inspect})
        file = SQLite3::Database.new(#{db_file.inspect}, readonly: true)
        backup = SQLite3::Backup.new(mem, "main", file, "main")
        backup.step(-1)
        backup.finish
        file.close
        mem.execute("ANALYZE")
        # The shared-cache in-memory database lives as long as one
        # connection to it stays open.
        $lowered_db_keepalive = mem
      RUBY
      @box = box
      $stderr.puts "lowered app #{tree} loaded in a Ruby::Box"
      at_exit { $stderr.puts "lowered bridge: #{@stats.sort_by { |_, v| -v }.to_h}" }
    end
  end

  class Middleware
    def initialize(app)
      @app = app
    end

    def call(env)
      box = LobstersFutamuraBridge.box
      return @app.call(env) unless box && env["REQUEST_METHOD"] == "GET"

      session = env["rack.session"]
      before = session.to_hash
      status, headers, body = box::Main.run_rack(lowered_env(box, env, before))
      if status == 404 || status >= 500
        LobstersFutamuraBridge.stats["fallback #{status} #{env['PATH_INFO']}"] += 1
        return @app.call(env)
      end
      LobstersFutamuraBridge.stats["lowered"] += 1
      write_back(box, session, before, headers)
      [status, headers, body]
    end

    private

    # The request as the lowered app expects it: the same env, with
    # Rails' session re-encoded as the lowered app's signed session
    # cookie. Escaped as a browser would send it.
    def lowered_env(box, env, values)
      lowered = box::ActionDispatch::Session.new
      values.each { |k, v| lowered[k.to_s] = v if v.is_a?(String) }
      signed = box::ActionDispatch::Session.signed_cookie(lowered.to_cookie, SESSION_KEY)
      cookies = env["HTTP_COOKIE"].to_s.split(/;\s*/).reject { |c| c.start_with?("#{SESSION_KEY}=") }
      env.merge("HTTP_COOKIE" => (cookies << "#{SESSION_KEY}=#{CGI.escape(signed)}").join("; "))
    end

    # Copy the lowered app's session changes into Rails' session and
    # drop its session Set-Cookie; Rails' CookieStore writes the real
    # one on the way out. Other cookies pass through.
    def write_back(box, session, before, headers)
      set_cookie = headers["set-cookie"] or return
      list = set_cookie.is_a?(Array) ? set_cookie : set_cookie.split("\n")
      mine, rest = list.partition { |c| c.start_with?("#{SESSION_KEY}=") }
      mine.each do |cookie|
        value = CGI.unescape(cookie[/\A#{SESSION_KEY}=([^;]*)/o, 1].to_s)
        after = box::ActionDispatch::Session.from_signed_cookie(value, SESSION_KEY).to_h
        after.each { |k, v| session[k] = v unless before[k.to_s] == v }
        (before.keys.map(&:to_s) - after.keys.map(&:to_s) - ["session_id"]).each { |k| session.delete(k) }
        LobstersFutamuraBridge.stats["session writes"] += 1
      end
      rest.empty? ? headers.delete("set-cookie") : headers["set-cookie"] = rest
    end
  end
end
